//! Tones mode (the warp modes spec, R18 and R20): pitch-synchronous overlap-add
//! for pitched material.
//!
//! Grains span a whole number of pitch periods, as near Grain Size as allowed,
//! placed whole periods apart so a steady tone keeps its pitch and waveform.
//! Unpitched audio falls back to Grain Size-long grains. A pitch change resamples
//! each grain by the pitch ratio through HiQ's interpolator, taking effect from
//! the next grain.

use crate::granular::{ActiveGrains, Grain, render_frame};
use crate::interp::Interpolation;
use crate::mode::{Request, WarpMode, Window, read};

/// Default Grain Size for Tones: 30 ms (R2).
pub const DEFAULT_TONES_GRAIN_MS: f64 = 30.0;
/// Minimum Grain Size for Tones: 10 ms (R2).
pub const MIN_TONES_GRAIN_MS: f64 = 10.0;
/// Maximum Grain Size for Tones: 100 ms (R2).
pub const MAX_TONES_GRAIN_MS: f64 = 100.0;

/// Finds the fundamental pitch period in samples (at `rate`) from the audio in
/// `signal`. Returns `Some(period)` if a clear pitch is detected, or `None` if
/// unpitched or silent.
#[must_use]
pub fn detect_period_fractional(signal: &[f32], rate: f64) -> Option<f64> {
    if signal.len() < 256 {
        return None;
    }
    let energy: f32 = signal.iter().map(|&s| s * s).sum();
    let rms = (energy / signal.len() as f32).sqrt();
    if rms < 0.001 {
        return None;
    }

    let min_lag = ((rate / 2000.0).round() as usize).max(8);
    let max_lag = ((rate / 50.0).round() as usize)
        .min(signal.len() / 2)
        .min(1200);
    if min_lag >= max_lag {
        return None;
    }

    let window_len = signal.len() - max_lag;
    if window_len < 64 {
        return None;
    }

    // Stack-allocated scratch buffer for the difference function (zero allocation).
    let mut d = [0.0f64; 1201];
    for tau in min_lag..=max_lag {
        let mut diff_sum = 0.0f64;
        for j in 0..window_len {
            let diff = f64::from(signal[j] - signal[j + tau]);
            diff_sum += diff * diff;
        }
        d[tau] = diff_sum;
    }

    let mut d_prime = [1.0f64; 1201];
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

    let tau_int = found_tau?;
    if tau_int > min_lag && tau_int < max_lag {
        let alpha = d_prime[tau_int - 1];
        let beta = d_prime[tau_int];
        let gamma = d_prime[tau_int + 1];
        let denom = alpha - 2.0 * beta + gamma;
        let delta = if denom.abs() > 1e-12 {
            0.5 * (alpha - gamma) / denom
        } else {
            0.0
        };
        Some(tau_int as f64 + delta)
    } else {
        Some(tau_int as f64)
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

        let floor = base_pos.floor() as i64;
        let at = (floor - window.first).max(0) as usize;
        let slice = if at < window.frames {
            let avail = (window.frames - at).min(2048);
            &window.channel(0)[at..at + avail]
        } else {
            &[]
        };
        let detected_frac = detect_period_fractional(slice, self.sample_rate);
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

        if self.last_frame.is_none_or(|last| last != start) {
            self.reset();
            self.prime_at(start, request, window);
        }

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
                t,
                i,
                out,
                stride,
            );
        }
        self.last_frame = Some(end);
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
    }
}
