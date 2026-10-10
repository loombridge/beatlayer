//! The scripted `--bench` scenario and its frame statistics.

use std::fmt;
use std::io::Write as _;
use std::path::Path;

/// The view starts zoomed in to eight bars and zooms out to the whole
/// timeline and back every [`ZOOM_PERIOD`] seconds.
pub const ZOOM_IN_BEATS: f64 = 32.0;
pub const ZOOM_PERIOD: f64 = 5.0;
/// Frames slower than this miss 60 fps.
pub const BUDGET_MS: f64 = 16.7;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Not benchmarking.
    Off,
    /// The first second: the scene settles, nothing is recorded.
    Warmup,
    /// Recording; seconds since the scenario started.
    Running(f64),
    /// The scenario is over.
    Done,
}

#[derive(Clone, Debug)]
pub struct Script {
    pub duration: f64,
    pub warmup: f64,
    /// Shrink the tracks so all 200 are on screen at once.
    pub all_tracks: bool,
    start: Option<f64>,
}

impl Script {
    pub fn new(duration: f64, all_tracks: bool) -> Self {
        Self {
            duration,
            warmup: 1.0,
            all_tracks,
            start: None,
        }
    }

    /// The phase at egui time `now` (seconds).
    pub fn phase(&mut self, now: f64) -> Phase {
        let start = *self.start.get_or_insert(now);
        let t = now - start - self.warmup;
        if t < 0.0 {
            Phase::Warmup
        } else if t < self.duration {
            Phase::Running(t)
        } else {
            Phase::Done
        }
    }

    /// The view at `t` seconds into the scenario: (first visible beat, points
    /// per beat). The zoom follows a raised cosine on a log scale; the left
    /// edge sweeps from the start of the timeline to the end over the run.
    pub fn view_at(&self, t: f64, total_beats: f64, timeline_width: f64) -> (f64, f64) {
        let mix = 0.5 * (1.0 - (std::f64::consts::TAU * t / ZOOM_PERIOD).cos());
        let visible = ZOOM_IN_BEATS * (total_beats / ZOOM_IN_BEATS).powf(mix);
        let progress = (t / self.duration).clamp(0.0, 1.0);
        let left = progress * (total_beats - visible).max(0.0);
        (left, timeline_width / visible)
    }
}

/// One frame's timing, in milliseconds.
#[derive(Clone, Copy, Debug)]
pub struct FrameSample {
    /// Time since the previous frame started.
    pub interval: f64,
    /// CPU time the main thread spent on this frame.
    pub cpu: f64,
    /// Wall time from frame start to after present (includes vsync waits).
    pub wall: f64,
}

#[derive(Clone, Debug)]
pub struct Summary {
    pub frames: usize,
    pub seconds: f64,
    pub interval_median: f64,
    pub interval_p99: f64,
    pub interval_max: f64,
    pub cpu_median: f64,
    pub cpu_p99: f64,
    pub cpu_max: f64,
    pub over_budget: usize,
}

/// Nearest-rank percentile of sorted data.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub fn summarise(samples: &[FrameSample]) -> Summary {
    let mut intervals: Vec<f64> = samples.iter().map(|s| s.interval).collect();
    let mut cpu: Vec<f64> = samples.iter().map(|s| s.cpu).collect();
    intervals.sort_by(f64::total_cmp);
    cpu.sort_by(f64::total_cmp);
    Summary {
        frames: samples.len(),
        seconds: intervals.iter().sum::<f64>() / 1000.0,
        interval_median: percentile(&intervals, 50.0),
        interval_p99: percentile(&intervals, 99.0),
        interval_max: intervals.last().copied().unwrap_or(f64::NAN),
        cpu_median: percentile(&cpu, 50.0),
        cpu_p99: percentile(&cpu, 99.0),
        cpu_max: cpu.last().copied().unwrap_or(f64::NAN),
        over_budget: intervals.iter().filter(|&&i| i > BUDGET_MS).count(),
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "frames                 {}", self.frames)?;
        writeln!(
            f,
            "mean rate              {:.1} fps",
            self.frames as f64 / self.seconds
        )?;
        writeln!(f, "frame interval median  {:.2} ms", self.interval_median)?;
        writeln!(f, "frame interval p99     {:.2} ms", self.interval_p99)?;
        writeln!(f, "frame interval max     {:.2} ms", self.interval_max)?;
        writeln!(f, "CPU frame time median  {:.2} ms", self.cpu_median)?;
        writeln!(f, "CPU frame time p99     {:.2} ms", self.cpu_p99)?;
        writeln!(f, "CPU frame time max     {:.2} ms", self.cpu_max)?;
        write!(
            f,
            "frames over {BUDGET_MS} ms   {} ({:.2}%)",
            self.over_budget,
            100.0 * self.over_budget as f64 / self.frames.max(1) as f64
        )
    }
}

pub fn write_csv(path: &Path, samples: &[FrameSample]) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(out, "frame,interval_ms,cpu_ms,wall_ms")?;
    for (i, s) in samples.iter().enumerate() {
        writeln!(out, "{i},{:.4},{:.4},{:.4}", s.interval, s.cpu, s.wall)?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles() {
        let data: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&data, 50.0), 50.0);
        assert_eq!(percentile(&data, 99.0), 99.0);
        assert_eq!(percentile(&data, 100.0), 100.0);
        assert_eq!(percentile(&[7.0], 99.0), 7.0);
    }

    #[test]
    fn summary_counts_slow_frames() {
        let samples: Vec<FrameSample> = [8.3, 8.4, 16.6, 16.8, 33.4]
            .iter()
            .map(|&ms| FrameSample {
                interval: ms,
                cpu: ms / 4.0,
                wall: ms,
            })
            .collect();
        let s = summarise(&samples);
        assert_eq!(s.over_budget, 2);
        assert_eq!(s.interval_median, 16.6);
        assert_eq!(s.interval_max, 33.4);
    }

    #[test]
    fn the_script_sweeps_the_timeline_and_cycles_the_zoom() {
        let mut script = Script::new(30.0, false);
        assert_eq!(script.phase(10.0), Phase::Warmup);
        assert_eq!(script.phase(11.5), Phase::Running(0.5));
        assert_eq!(script.phase(41.0), Phase::Done);

        let total = 2048.0;
        let width = 1000.0;
        let (left, ppb) = script.view_at(0.0, total, width);
        assert_eq!(left, 0.0);
        assert!((width / ppb - ZOOM_IN_BEATS).abs() < 1e-9);
        let (_, ppb) = script.view_at(2.5, total, width);
        assert!(
            (width / ppb - total).abs() < 1e-6,
            "fully zoomed out at 2.5 s"
        );
        let (left, ppb) = script.view_at(30.0, total, width);
        assert!(
            (left + width / ppb - total).abs() < 1e-6,
            "ends at the last bar"
        );
    }
}
