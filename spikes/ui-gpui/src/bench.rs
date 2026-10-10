//! The scripted bench scenario and its frame statistics.

use std::fmt::Write as _;
use std::time::Instant;

use crate::data::BARS;
use crate::platform;

/// Seconds before recording starts, so start-up work (shader compilation,
/// glyph atlas warm-up) doesn't count.
pub const SETTLE_SECONDS: f64 = 1.0;
const BASE_PX_PER_BAR: f32 = 24.0;
const ZOOM_PERIOD: f64 = 5.0;
const ZOOM_RANGE: f32 = 4.0;

/// The arrangement viewport at `t` seconds into the scenario: horizontal
/// scroll from the first bar to the last over `duration`, while the zoom goes
/// from 24 to 96 pixels per bar and back every 5 seconds.
pub fn scenario(t: f64, duration: f64, lanes_width: f32) -> (f64, f32) {
    let phase = (1.0 - (std::f64::consts::TAU * t / ZOOM_PERIOD).cos()) / 2.0;
    let px_per_bar = BASE_PX_PER_BAR * ZOOM_RANGE.powf(phase as f32);
    let visible = (lanes_width / px_per_bar) as f64;
    let progress = (t / duration).clamp(0.0, 1.0);
    let scroll_x = progress * (BARS as f64 - visible).max(0.0);
    (scroll_x, px_per_bar)
}

#[derive(Default)]
pub struct Recorder {
    last: Option<(Instant, f64, f64)>,
    /// Wall-clock time between consecutive frames, in ms.
    pub intervals: Vec<f64>,
    /// Main-thread CPU time between consecutive frames, in ms.
    pub main_cpu: Vec<f64>,
    /// Whole-process CPU time between consecutive frames, in ms.
    pub process_cpu: Vec<f64>,
    pub quads: Vec<usize>,
}

impl Recorder {
    /// Records a frame boundary. Called from the frame callback GPUI runs just
    /// before it draws, so the span between two calls is one frame: the draw
    /// and present of the previous frame plus any event handling.
    pub fn frame(&mut self, now: Instant, quads: usize) {
        let (thread, process) = platform::cpu_times();
        if let Some((t0, th0, pr0)) = self.last {
            self.intervals
                .push(now.duration_since(t0).as_secs_f64() * 1e3);
            self.main_cpu.push((thread - th0) * 1e3);
            self.process_cpu.push((process - pr0) * 1e3);
            self.quads.push(quads);
        }
        self.last = Some((now, thread, process));
    }

    pub fn report(&self, duration: f64, scale: f32, lanes: (f32, f32)) -> String {
        let mut out = String::new();
        let n = self.intervals.len();
        let over = self.intervals.iter().filter(|&&ms| ms > 16.7).count();
        let _ = writeln!(
            out,
            "bench: {duration:.0} s scripted scenario (horizontal scroll across 512 bars, zoom cycle every 5 s, 200 meters animating)"
        );
        let _ = writeln!(
            out,
            "scale factor: {scale}, lanes area: {:.0} x {:.0} pt",
            lanes.0, lanes.1
        );
        let _ = writeln!(out, "frames: {n} ({:.1} per second)", n as f64 / duration);
        let _ = writeln!(
            out,
            "frame interval ms: median {:.2}, p99 {:.2}, max {:.2}",
            percentile(&self.intervals, 50.0),
            percentile(&self.intervals, 99.0),
            max(&self.intervals)
        );
        let _ = writeln!(
            out,
            "main-thread CPU per frame ms: median {:.2}, p99 {:.2}, max {:.2}",
            percentile(&self.main_cpu, 50.0),
            percentile(&self.main_cpu, 99.0),
            max(&self.main_cpu)
        );
        let _ = writeln!(
            out,
            "process CPU per frame ms: median {:.2}, p99 {:.2}, max {:.2}",
            percentile(&self.process_cpu, 50.0),
            percentile(&self.process_cpu, 99.0),
            max(&self.process_cpu)
        );
        let _ = writeln!(out, "frames over 16.7 ms: {over}");
        let stalls = self.intervals.iter().filter(|&&ms| ms > 1000.0).count();
        if stalls > 0 {
            let _ = writeln!(
                out,
                "INVALID RUN: {stalls} frame gaps over 1 s (window hidden or display asleep)"
            );
        }
        let quads: Vec<f64> = self.quads.iter().map(|&q| q as f64).collect();
        let _ = writeln!(
            out,
            "arrangement quads per frame: median {:.0}, max {:.0}",
            percentile(&quads, 50.0),
            max(&quads)
        );
        out
    }

    pub fn csv(&self) -> String {
        let mut out = String::from("frame,interval_ms,main_cpu_ms,process_cpu_ms,quads\n");
        for i in 0..self.intervals.len() {
            let _ = writeln!(
                out,
                "{i},{:.4},{:.4},{:.4},{}",
                self.intervals[i], self.main_cpu[i], self.process_cpu[i], self.quads[i]
            );
        }
        out
    }
}

/// Nearest-rank percentile.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize;
    v[rank.min(v.len()) - 1]
}

fn max(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::NAN, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 50.0), 50.0);
        assert_eq!(percentile(&v, 99.0), 99.0);
        assert_eq!(percentile(&[3.0], 99.0), 3.0);
    }

    #[test]
    fn scenario_covers_the_song_and_cycles_zoom() {
        let (x0, z0) = scenario(0.0, 30.0, 1100.0);
        assert_eq!(x0, 0.0);
        assert!((z0 - 24.0).abs() < 1e-3);
        let (_, z_mid) = scenario(2.5, 30.0, 1100.0);
        assert!((z_mid - 96.0).abs() < 1e-3);
        let (_, z5) = scenario(5.0, 30.0, 1100.0);
        assert!((z5 - 24.0).abs() < 1e-3);
        let (x_end, z_end) = scenario(30.0, 30.0, 1100.0);
        assert!((x_end + (1100.0 / z_end) as f64 - 512.0).abs() < 1e-6);
    }
}
