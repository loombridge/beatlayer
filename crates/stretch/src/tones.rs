//! Tones mode (the warp modes spec, R18 and R20): pitch-synchronous overlap-add
//! for pitched material.
//!
//! Grains span a whole number of pitch periods, as near Grain Size as allowed,
//! placed whole periods apart so a steady tone keeps its pitch and waveform.
//! Unpitched audio falls back to Grain Size-long grains. A pitch change resamples
//! each grain by the pitch ratio through HiQ's interpolator, taking effect from
//! the next grain.

use std::cell::Cell;

use crate::granular::{ActiveGrains, Grain, render_frame};
use crate::interp::{Interpolation, Taps};
use crate::mode::{Request, WarpMode, Window, read};

/// Default Grain Size for Tones: 30 ms (R2).
pub const DEFAULT_TONES_GRAIN_MS: f64 = 30.0;
/// Minimum Grain Size for Tones: 10 ms (R2).
pub const MIN_TONES_GRAIN_MS: f64 = 10.0;
/// Maximum Grain Size for Tones: 100 ms (R2).
pub const MAX_TONES_GRAIN_MS: f64 = 100.0;

thread_local! {
    static DIFF_OPS: Cell<u64> = const { Cell::new(0) };
}

/// How many multiply-adds this thread's pitch searches have done, for tests
/// that bound a block's work (R5).
#[must_use]
pub fn diff_ops() -> u64 {
    DIFF_OPS.with(Cell::get)
}

fn count_ops(ops: usize) {
    DIFF_OPS.with(|total| total.set(total.get() + ops as u64));
}

/// The rate the pitch search runs at, about: the signal is decimated to it, so
/// the search costs the same and reaches the same 50 Hz floor at every engine
/// rate.
const SEARCH_RATE: f64 = 12_000.0;
/// The longest lag the search weighs, in decimated samples: 50 Hz at the
/// slowest rate the decimation leaves undivided.
const MAX_LAG: usize = 360;

/// The squared difference between `signal` and itself `lag` later, over `len` frames.
fn difference(signal: &[f32], lag: usize, len: usize) -> f64 {
    (0..len)
        .map(|j| f64::from(signal[j] - signal[j + lag]).powi(2))
        .sum()
}

/// Finds the fundamental pitch period in samples (at `rate`) from the audio in
/// `signal`. Returns `Some(period)` if a clear pitch is detected, or `None` if
/// unpitched or silent. Reads at most two periods of 50 Hz of `signal`: it
/// searches a copy decimated to about 12 kHz, then settles the period at `rate`
/// among the lags next to the one it found.
#[must_use]
pub fn detect_period_fractional(signal: &[f32], rate: f64) -> Option<f64> {
    let signal = &signal[..signal.len().min(2 * (rate / 50.0).round() as usize + 64)];
    if signal.len() < 256 {
        return None;
    }
    let energy: f32 = signal.iter().map(|&s| s * s).sum();
    let rms = (energy / signal.len() as f32).sqrt();
    if rms < 0.001 {
        return None;
    }

    // Box-averages `step` frames into one: stack scratch, no allocation.
    let step = (rate / SEARCH_RATE).round().max(1.0) as usize;
    let mut decimated = [0.0f32; 2 * MAX_LAG + 80];
    let n = (signal.len() / step).min(decimated.len());
    for (out, frames) in decimated.iter_mut().zip(signal.chunks_exact(step)) {
        *out = frames.iter().sum::<f32>() / step as f32;
    }
    let x = &decimated[..n];

    let min_lag = ((rate / 2000.0 / step as f64).round() as usize).max(2);
    let max_lag = ((rate / 50.0 / step as f64).round() as usize)
        .min(n / 2)
        .min(MAX_LAG);
    if min_lag >= max_lag {
        return None;
    }

    let window_len = n - max_lag;
    if window_len < 64 {
        return None;
    }

    count_ops((max_lag - min_lag + 1) * window_len);
    let mut d = [0.0f64; MAX_LAG + 1];
    for (tau, slot) in d.iter_mut().enumerate().take(max_lag + 1).skip(min_lag) {
        *slot = difference(x, tau, window_len);
    }

    let mut d_prime = [1.0f64; MAX_LAG + 1];
    let mut run_sum = 0.0f64;
    for tau in 1..=max_lag {
        run_sum += d[tau];
        d_prime[tau] = if run_sum > 1e-12 {
            d[tau] * tau as f64 / run_sum
        } else {
            1.0
        };
    }

    const THRESHOLD: f64 = 0.10;
    let mut tau = min_lag;
    let mut found_tau = None;
    while tau < max_lag {
        if d_prime[tau] < THRESHOLD {
            while tau < max_lag && d_prime[tau + 1] < d_prime[tau] {
                tau += 1;
            }
            found_tau = Some(tau);
            break;
        }
        tau += 1;
    }

    // The lag the decimated search found is good to a `step`: take the
    // deepest dip of the undecimated difference within one either side, and
    // place the period between its neighbours.
    let centre = found_tau? * step;
    let (low, high) = (centre - step, centre + step);
    let len = signal.len().saturating_sub(high + 1);
    if len < 64 {
        return Some(centre as f64);
    }
    count_ops((2 * step + 3) * len);
    let (mut lag, mut dip) = (centre, f64::MAX);
    for candidate in low..=high {
        let value = difference(signal, candidate, len);
        if value < dip {
            (lag, dip) = (candidate, value);
        }
    }
    let alpha = difference(signal, lag - 1, len);
    let gamma = difference(signal, lag + 1, len);
    let denom = alpha - 2.0 * dip + gamma;
    let delta = if denom > 1e-12 {
        (0.5 * (alpha - gamma) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    Some(lag as f64 + delta)
}

/// The pitch period a granular mode found lately, so a grain whose read point is
/// near the last search's takes its period and does not search again. Depends
/// only on read points, never on how a render was split into blocks (R6).
#[derive(Clone, Debug, Default)]
pub(crate) struct PeriodCache {
    /// The file position of the last search, and the period it found.
    last: Option<(f64, f64)>,
}

impl PeriodCache {
    /// The period of the audio at file position `pos`: the last search's if
    /// that was within four `hop`s or periods, else a new search.
    pub(crate) fn period(
        &mut self,
        window: &Window<'_>,
        pos: f64,
        rate: f64,
        hop: usize,
    ) -> Option<f64> {
        if let Some((at, period)) = self.last
            && (pos - at).abs() < 4.0 * period.max(hop as f64)
        {
            return Some(period);
        }
        let at = (pos.floor() as i64 - window.first).max(0) as usize;
        let slice = if at < window.frames {
            &window.channel(0)[at..]
        } else {
            &[]
        };
        self.last = detect_period_fractional(slice, rate).map(|period| (pos, period));
        self.last.map(|(_, period)| period)
    }
}

/// Detects the fundamental pitch period in samples using the YIN difference algorithm.
/// Returns `None` if the signal is unpitched.
#[must_use]
pub fn detect_period(signal: &[f32], sample_rate: f64) -> Option<usize> {
    detect_period_fractional(signal, sample_rate).map(|p| p.round() as usize)
}

/// Computes grain length and synthesis hop in frames for a given nominal grain size
/// and optional detected pitch period.
#[must_use]
pub fn grain_for_period(nominal_grain_size: usize, period: Option<usize>) -> (usize, usize) {
    match period {
        Some(p) if p > 0 => {
            let k = ((nominal_grain_size as f64 / p as f64).round() as usize).max(1);
            let len = k * p;
            let m = (k / 2).max(1);
            let hop = m * p;
            (len, hop)
        }
        _ => (nominal_grain_size, (nominal_grain_size / 2).max(1)),
    }
}

/// Tones warp mode (R18).
#[derive(Clone, Debug)]
pub struct Tones {
    grain_size_ms: f64,
    sample_rate: f64,
    active_grains: ActiveGrains,
    next_grain_start: i64,
    last_frame: Option<i64>,
    last_phase_pos: Option<f64>,
    clip: u64,
    periods: PeriodCache,
}

impl Default for Tones {
    fn default() -> Self {
        Self::new(2, 48_000.0)
    }
}

impl Tones {
    /// A new Tones stretcher for `sample_rate`.
    #[must_use]
    pub fn new(_channels: usize, sample_rate: f64) -> Self {
        Self {
            grain_size_ms: DEFAULT_TONES_GRAIN_MS,
            sample_rate: sample_rate.max(1.0),
            active_grains: ActiveGrains::default(),
            next_grain_start: 0,
            last_frame: None,
            last_phase_pos: None,
            clip: 0,
            periods: PeriodCache::default(),
        }
    }

    /// Sets Grain Size in milliseconds (10 to 100 ms).
    pub fn set_grain_size(&mut self, ms: f64) {
        self.grain_size_ms = ms.clamp(MIN_TONES_GRAIN_MS, MAX_TONES_GRAIN_MS);
    }

    /// Primes initial overlapping grains anchored at song frame `start`.
    fn prime_at(&mut self, start: i64, request: &Request<'_>, window: &Window<'_>) {
        self.next_grain_start = start;
        self.spawn_grain(window, request, request.positions[0]);
    }

    /// Spawns the next grain according to pitch analysis around the current read point.
    fn spawn_grain(&mut self, window: &Window<'_>, request: &Request<'_>, pos_at_grain: f64) {
        let g_nominal =
            ((self.grain_size_ms * self.sample_rate / 1000.0).round() as usize).clamp(16, 9600);

        let grain_t = self.next_grain_start;
        let start = request.first;
        let end = start + request.positions.len() as i64;
        let base_pos = if grain_t >= start && grain_t < end {
            request.positions[(grain_t - start) as usize]
        } else {
            pos_at_grain + (grain_t - start) as f64 * request.speed
        };

        let detected_frac = self
            .periods
            .period(window, base_pos, self.sample_rate, g_nominal / 2);
        let detected = detected_frac.map(|p| p.round() as usize);
        let (len, hop) = grain_for_period(g_nominal, detected);

        let pitch_ratio = request.pitch.max(0.01);
        let out_len = ((len as f64) / pitch_ratio).round() as usize;
        let out_hop = (((hop as f64) / pitch_ratio).round() as usize).max(1);

        let file_pos = if let Some(p) = detected_frac {
            let pos = if let Some(last_pos) = self.last_phase_pos {
                let expected = last_pos + out_hop as f64 * pitch_ratio;
                let delta = base_pos - expected;
                let k = (delta / p).round();
                expected + k * p
            } else {
                base_pos
            };
            self.last_phase_pos = Some(pos);
            pos
        } else {
            self.last_phase_pos = None;
            base_pos
        };

        self.active_grains.push(Grain {
            start: grain_t,
            len: out_len.max(4),
            file_pos,
            pitch_ratio,
        });
        self.next_grain_start += out_hop as i64;
    }

    /// Renders frames via granular overlap-add.
    fn render_granular(
        &mut self,
        request: &Request<'_>,
        window: &Window<'_>,
        out: &mut [f32],
        stride: usize,
    ) {
        let n = request.positions.len();
        if n == 0 {
            return;
        }
        let start = request.first;
        let end = start + n as i64;
        let interpolation = Interpolation::of(request.high_quality);

        if self.clip != request.clip || self.last_frame.is_none_or(|last| last != start) {
            self.reset();
            self.prime_at(start, request, window);
        }

        let mut taps = Taps::new();
        for (i, &pos_i) in request.positions.iter().enumerate() {
            let t = start + i as i64;
            while self.next_grain_start <= t {
                self.spawn_grain(window, request, pos_i);
            }
            self.active_grains.prune(t);
            render_frame(
                &self.active_grains,
                window,
                interpolation,
                &mut taps,
                t,
                &mut out[i..],
                stride,
            );
        }
        self.last_frame = Some(end);
        self.clip = request.clip;
    }
}

impl WarpMode for Tones {
    fn reach(&self, request: &Request<'_>) -> (usize, usize) {
        if request.neutral {
            return (0, 0);
        }
        let g = (self.grain_size_ms * self.sample_rate / 1000.0).ceil() as usize;
        let interp = Interpolation::of(request.high_quality)
            .reach(request.speed)
            .0;
        let reach = g * 2 + interp + 64;
        (reach, reach)
    }

    // Spec: warp-modes R18, R20
    fn render(
        &mut self,
        request: &Request<'_>,
        window: &Window<'_>,
        out: &mut [f32],
        stride: usize,
    ) {
        if request.neutral {
            read(request, window, out, stride);
            self.reset();
            return;
        }
        self.render_granular(request, window, out, stride);
    }

    fn reset(&mut self) {
        self.active_grains.clear();
        self.last_frame = None;
        self.last_phase_pos = None;
        self.periods = PeriodCache::default();
    }
}
