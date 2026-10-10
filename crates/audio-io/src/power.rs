//! Noticing that the computer slept and woke.
//!
//! A [`Supervisor`](crate::Supervisor) asks a [`PowerMonitor`] on every poll
//! whether the machine is going to sleep or has woken. The default monitor,
//! [`ClockJump`], works the same on every platform: it reads two clocks on
//! each poll and notices a wake from a jump between them afterwards. A
//! monitor fed by the operating system's own notifications (IOKit on macOS,
//! power broadcasts on Windows, logind on Linux) would also say when the
//! machine is about to sleep, so the stream can stop first; it plugs in
//! through the same trait.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What the machine's power state did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    /// The machine is about to sleep.
    Sleeping,
    /// The machine has woken.
    Woke {
        /// How long it slept, if the monitor knows.
        slept: Option<Duration>,
    },
}

/// Where a [`Supervisor`](crate::Supervisor) learns that the machine slept
/// and woke. It's polled on the supervisor's own thread, never an audio
/// thread.
pub trait PowerMonitor: Send {
    /// The next thing that happened since the last poll, if anything.
    fn poll(&mut self) -> Option<PowerEvent>;
}

/// The default threshold for [`ClockJump`]: a jump of more than this between
/// two polls counts as a sleep.
pub const DEFAULT_SLEEP_THRESHOLD: Duration = Duration::from_secs(3);

/// One reading of the two clocks [`ClockJump`] compares: a monotonic clock
/// and the wall clock, each as time since some fixed point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockReading {
    /// The monotonic clock (`Instant`).
    pub monotonic: Duration,
    /// The wall clock (`SystemTime`).
    pub wall: Duration,
}

/// Notices a wake after the fact, from the clocks.
///
/// On macOS and Linux the monotonic clock behind `Instant` stops while the
/// machine sleeps (`CLOCK_UPTIME_RAW` and `CLOCK_MONOTONIC`) and the wall
/// clock doesn't, so after a sleep the wall clock has moved further between
/// two polls than the monotonic clock. On Windows `Instant` keeps counting
/// through sleep, so the gap between two polls is itself the sign. Either
/// gap over the threshold reports [`PowerEvent::Woke`] with its length.
///
/// The wall clock being set forward, or the polling thread being starved for
/// longer than the threshold, reads the same way. That's harmless: the
/// supervisor answers a wake only by checking that its stream still runs.
pub struct ClockJump {
    threshold: Duration,
    clock: Box<dyn FnMut() -> ClockReading + Send>,
    last: Option<ClockReading>,
}

impl ClockJump {
    /// A monitor on the system's clocks.
    #[must_use]
    pub fn new(threshold: Duration) -> Self {
        let origin = Instant::now();
        Self::with_clock(threshold, move || ClockReading {
            monotonic: origin.elapsed(),
            wall: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default(),
        })
    }

    /// A monitor on clocks of the caller's, for tests.
    #[must_use]
    pub fn with_clock(
        threshold: Duration,
        clock: impl FnMut() -> ClockReading + Send + 'static,
    ) -> Self {
        Self {
            threshold,
            clock: Box::new(clock),
            last: None,
        }
    }
}

impl Default for ClockJump {
    fn default() -> Self {
        Self::new(DEFAULT_SLEEP_THRESHOLD)
    }
}

impl std::fmt::Debug for ClockJump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClockJump")
            .field("threshold", &self.threshold)
            .field("last", &self.last)
            .finish_non_exhaustive()
    }
}

impl PowerMonitor for ClockJump {
    fn poll(&mut self) -> Option<PowerEvent> {
        let now = (self.clock)();
        let last = self.last.replace(now)?;
        let monotonic = now.monotonic.saturating_sub(last.monotonic);
        // A wall clock set backwards gives nothing here.
        let missing = now.wall.saturating_sub(last.wall).saturating_sub(monotonic);
        let gap = missing.max(monotonic);
        (gap > self.threshold).then_some(PowerEvent::Woke { slept: Some(gap) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A clock the test moves by hand.
    fn manual() -> (ClockJump, Arc<Mutex<ClockReading>>) {
        let reading = Arc::new(Mutex::new(ClockReading {
            monotonic: Duration::ZERO,
            wall: Duration::from_secs(1_000_000),
        }));
        let shared = Arc::clone(&reading);
        let monitor =
            ClockJump::with_clock(Duration::from_secs(3), move || *shared.lock().unwrap());
        (monitor, reading)
    }

    fn advance(reading: &Mutex<ClockReading>, monotonic_ms: u64, wall_ms: u64) {
        let mut reading = reading.lock().unwrap();
        reading.monotonic += Duration::from_millis(monotonic_ms);
        reading.wall += Duration::from_millis(wall_ms);
    }

    // Spec: device-changes R4
    #[test]
    fn steady_polls_report_nothing() {
        let (mut monitor, reading) = manual();
        for _ in 0..100 {
            assert_eq!(monitor.poll(), None);
            // Polls 50 ms apart, the wall clock drifting a little.
            advance(&reading, 50, 52);
        }
    }

    // Spec: device-changes R4
    #[test]
    fn the_wall_clock_running_ahead_is_a_sleep() {
        // macOS and Linux: the monotonic clock stops while asleep.
        let (mut monitor, reading) = manual();
        monitor.poll();
        advance(&reading, 50, 600_050);
        assert_eq!(
            monitor.poll(),
            Some(PowerEvent::Woke {
                slept: Some(Duration::from_secs(600))
            })
        );
        advance(&reading, 50, 50);
        assert_eq!(monitor.poll(), None, "reported once");
    }

    // Spec: device-changes R4
    #[test]
    fn a_long_gap_on_both_clocks_is_a_sleep() {
        // Windows: `Instant` counts through sleep.
        let (mut monitor, reading) = manual();
        monitor.poll();
        advance(&reading, 90_000, 90_000);
        assert_eq!(
            monitor.poll(),
            Some(PowerEvent::Woke {
                slept: Some(Duration::from_secs(90))
            })
        );
    }

    // Spec: device-changes R4
    #[test]
    fn gaps_under_the_threshold_and_clocks_set_back_are_ignored() {
        let (mut monitor, reading) = manual();
        monitor.poll();
        advance(&reading, 2_500, 2_900);
        assert_eq!(monitor.poll(), None);
        // The wall clock set back an hour.
        reading.lock().unwrap().wall -= Duration::from_secs(3_600);
        advance(&reading, 50, 50);
        assert_eq!(monitor.poll(), None);
    }

    #[test]
    fn the_system_clocks_read_without_a_jump() {
        let mut monitor = ClockJump::default();
        assert_eq!(monitor.poll(), None);
        assert_eq!(monitor.poll(), None);
    }
}
