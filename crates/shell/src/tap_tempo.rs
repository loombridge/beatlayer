//! Tap Tempo's law without a window (the control bar transport spec, R18 and
//! R19): how a run of taps, each on a frame of the engine clock, becomes a
//! tempo. `Shell::run_tap_tempo` runs the command around it.

use std::collections::VecDeque;

use bl_shell_spec::transport::TapTempo;

use crate::transport::{TEMPO_MAX, TEMPO_MIN};

/// A tap more than this long after the run's last starts a new run: a beat
/// at 20 BPM, the slowest tempo (R18).
pub const RUN_GAP_SECONDS: f64 = 3.0;
/// A tap less than this long after the run's last is ignored as a bounce: a
/// beat faster than 999 BPM (R18).
pub const BOUNCE_SECONDS: f64 = 0.06;
/// How many of a run's last intervals the tempo is the mean of (R18).
pub const INTERVALS: usize = 8;

/// What a tap came to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tapped {
    /// A bounce, or a frame before the run's last tap: nothing changes.
    Ignored,
    /// The tap joined a run; from its second tap on, `mean_interval` is the
    /// mean of the last intervals, in frames.
    Counted { mean_interval: Option<f64> },
}

/// The run of taps Tap Tempo has now.
#[derive(Debug, Default)]
pub struct TapRun {
    /// The frames of the run's last taps, enough for [`INTERVALS`] intervals.
    recent: VecDeque<u64>,
    taps: u32,
    /// The tempo the run set.
    pub tempo: Option<f64>,
    /// The frame a tap start waits for.
    pub start_at: Option<u64>,
    /// Where the document and its history stood after the run's last tempo
    /// edit, while that edit is the run's undo step: the next tap amends the
    /// step if nothing else has happened since (R21).
    pub step: Option<(u64, usize)>,
}

impl TapRun {
    /// How many taps the run has.
    #[must_use]
    pub fn taps(&self) -> u32 {
        self.taps
    }

    /// Takes a tap on engine frame `frame`, the clock counting `rate` frames
    /// to the second: a new run if the last tap is more than
    /// [`RUN_GAP_SECONDS`] back.
    pub fn tap(&mut self, frame: u64, rate: u32) -> Tapped {
        let rate = f64::from(rate.max(1));
        if let Some(&last) = self.recent.back() {
            let since = frame.saturating_sub(last) as f64 / rate;
            if frame <= last || since < BOUNCE_SECONDS {
                return Tapped::Ignored;
            }
            if since > RUN_GAP_SECONDS {
                *self = Self::default();
            }
        }
        if self.recent.len() > INTERVALS {
            self.recent.pop_front();
        }
        self.recent.push_back(frame);
        self.taps += 1;
        let intervals = self.recent.len() - 1;
        let mean_interval = (intervals > 0).then(|| {
            // The intervals add up to the span from the first tap to the last.
            (frame - self.recent[0]) as f64 / intervals as f64
        });
        Tapped::Counted { mean_interval }
    }

    /// The run as `transport.state` and the command report it with the engine
    /// clock at `clock`: empty once a new run would start, and a start only
    /// while it still waits for its frame.
    #[must_use]
    pub fn report(&self, clock: u64, rate: u32) -> TapTempo {
        let ended = self.recent.back().is_none_or(|&last| {
            clock.saturating_sub(last) as f64 / f64::from(rate.max(1)) > RUN_GAP_SECONDS
        });
        TapTempo {
            taps: if ended { 0 } else { self.taps },
            tempo: if ended { None } else { self.tempo },
            start_at: self.start_at.filter(|&frame| frame > clock),
        }
    }
}

/// The tempo in quarter notes per minute of taps `mean_interval` frames
/// apart, one beat of a time signature with `denominator` each, to the
/// hundredth within the Tempo field's range (R18).
#[must_use]
pub fn tempo_of(mean_interval: f64, rate: u32, denominator: u8) -> f64 {
    let beats_per_minute = 60.0 * f64::from(rate) / mean_interval;
    let quarters = beats_per_minute * 4.0 / f64::from(denominator.max(1));
    ((quarters * 100.0).round() / 100.0).clamp(TEMPO_MIN, TEMPO_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const MS: u64 = 48;

    /// Taps at `gaps` milliseconds apart from frame 100,000, with the tempo
    /// each one sets.
    fn tapped(gaps: &[u64], denominator: u8) -> Vec<Option<f64>> {
        let mut run = TapRun::default();
        let mut frame = 100_000;
        let mut tempi = vec![];
        for gap in std::iter::once(&0).chain(gaps) {
            frame += gap * MS;
            tempi.push(match run.tap(frame, RATE) {
                Tapped::Counted {
                    mean_interval: Some(mean),
                } => Some(tempo_of(mean, RATE, denominator)),
                _ => None,
            });
        }
        tempi
    }

    #[test]
    fn taps_500_ms_apart_set_120_from_the_second() {
        // Spec: control-bar-transport R18
        assert_eq!(
            tapped(&[500, 500, 500], 4),
            [None, Some(120.0), Some(120.0), Some(120.0)]
        );
    }

    #[test]
    fn the_mean_is_of_the_last_eight_intervals() {
        // Spec: control-bar-transport R18. Nine taps, then a tenth that
        // drops the first interval (1,000 ms) from the mean.
        let nine = tapped(&[1000, 500, 500, 500, 500, 500, 500, 500], 4);
        // (1000 + 7 × 500) / 8 = 562.5 ms: 106.67 BPM.
        assert_eq!(nine[8], Some(106.67));
        let ten = tapped(&[1000, 500, 500, 500, 500, 500, 500, 500, 500], 4);
        assert_eq!(ten[9], Some(120.0));
    }

    #[test]
    fn a_pause_over_three_seconds_starts_a_new_run() {
        // Spec: control-bar-transport R18
        let mut run = TapRun::default();
        run.tap(100_000, RATE);
        run.tap(100_000 + 500 * MS, RATE);
        assert_eq!(run.taps(), 2);
        let later = 100_000 + 500 * MS + 3_500 * MS;
        assert_eq!(
            run.tap(later, RATE),
            Tapped::Counted {
                mean_interval: None
            }
        );
        assert_eq!(run.taps(), 1);
        assert_eq!(run.report(later, RATE).tempo, None);
    }

    #[test]
    fn a_tap_within_60_ms_is_ignored() {
        // Spec: control-bar-transport R18
        let mut run = TapRun::default();
        run.tap(100_000, RATE);
        assert_eq!(run.tap(100_000 + 30 * MS, RATE), Tapped::Ignored);
        assert_eq!(run.tap(99_000, RATE), Tapped::Ignored);
        assert_eq!(run.taps(), 1);
    }

    #[test]
    fn a_tap_is_one_beat_of_the_signature() {
        // Spec: control-bar-transport R18. Eighth notes 250 ms apart are 120
        // quarter notes a minute in 6/8.
        assert_eq!(tapped(&[250], 8)[1], Some(120.0));
    }

    #[test]
    fn the_tempo_stays_within_the_tempo_fields_range() {
        assert_eq!(tapped(&[3_000], 4)[1], Some(20.0));
        assert_eq!(tapped(&[61], 4)[1], Some(983.61));
        assert_eq!(tempo_of(1.0, RATE, 4), 999.0);
    }

    #[test]
    fn a_run_reports_empty_once_a_new_one_would_start() {
        let mut run = TapRun::default();
        run.tap(100_000, RATE);
        run.tap(100_000 + 500 * MS, RATE);
        run.tempo = Some(120.0);
        run.start_at = Some(100_000 + 1_000 * MS);
        let last = 100_000 + 500 * MS;
        let during = run.report(last + 100 * MS, RATE);
        assert_eq!((during.taps, during.tempo), (2, Some(120.0)));
        assert_eq!(during.start_at, Some(100_000 + 1_000 * MS));
        let after = run.report(last + 3_100 * MS, RATE);
        assert_eq!((after.taps, after.tempo, after.start_at), (0, None, None));
    }
}
