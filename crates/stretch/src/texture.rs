//! Texture mode (the warp modes spec, R19 and R20): granular time-stretching with Flux.
//!
//! Rebuilds audio from overlapping, crossfaded grains Grain Size long, without
//! pitch analysis. Flux jitters each grain's place and length by up to Flux's
//! share of Grain Size, seeded from the clip's ID so every render repeats (R6).
//! Resamples each grain by the pitch ratio through HiQ's interpolator (R20).

use bl_dsp::Random;

use crate::granular::{ActiveGrains, Grain, render_frame};
use crate::interp::{Interpolation, Taps};
use crate::mode::{Request, WarpMode, Window, read};
use crate::tones::PeriodCache;

/// Default Grain Size for Texture: 65 ms (R2).
pub const DEFAULT_TEXTURE_GRAIN_MS: f64 = 65.0;
/// Minimum Grain Size for Texture: 10 ms (R2).
pub const MIN_TEXTURE_GRAIN_MS: f64 = 10.0;
/// Maximum Grain Size for Texture: 500 ms (R2).
pub const MAX_TEXTURE_GRAIN_MS: f64 = 500.0;

/// Default Flux for Texture: 25 % (R2).
pub const DEFAULT_TEXTURE_FLUX: f64 = 25.0;
/// Minimum Flux: 0 %.
pub const MIN_TEXTURE_FLUX: f64 = 0.0;
/// Maximum Flux: 100 %.
pub const MAX_TEXTURE_FLUX: f64 = 100.0;

/// The most a Texture read reaches each way at `rate`, at Grain Size's and
/// Flux's maxima (R19): what a window around a run must hold, and what a
/// clip's source must keep ready. Kept in step with [`Texture::reach`], which
/// no setting's read may exceed.
#[must_use]
pub fn max_reach(rate: f64) -> usize {
    let g = (MAX_TEXTURE_GRAIN_MS / 1000.0 * rate).ceil() as usize;
    // Flux moves a grain's place in the file, and lengthens it, by up to its
    // share of Grain Size each way, so at Flux's maximum a read reaches this
    // far: twice the grain beyond the place the warp map reads.
    let jitter = g;
    let interp = Interpolation::of(true).reach(f64::INFINITY).1;
    2 * g + 2 * jitter + interp + 64
}

/// Texture warp mode (R19).
#[derive(Clone, Debug)]
pub struct Texture {
    grain_size_ms: f64,
    flux: f64,
    sample_rate: f64,
    seed: u64,
    random: Random,
    active_grains: ActiveGrains,
    next_grain_start: i64,
    last_frame: Option<i64>,
    last_grain: Option<(i64, f64)>,
    periods: PeriodCache,
}

impl Default for Texture {
    fn default() -> Self {
        Self::new(2, 48_000.0, 0)
    }
}

impl Texture {
    /// A new Texture stretcher for `sample_rate` and `seed`.
    #[must_use]
    pub fn new(_channels: usize, sample_rate: f64, seed: u64) -> Self {
        Self {
            grain_size_ms: DEFAULT_TEXTURE_GRAIN_MS,
            flux: DEFAULT_TEXTURE_FLUX,
            sample_rate: sample_rate.max(1.0),
            seed,
            random: Random::seeded(seed),
            active_grains: ActiveGrains::default(),
            next_grain_start: 0,
            last_frame: None,
            last_grain: None,
            periods: PeriodCache::default(),
        }
    }

    /// Sets Grain Size in milliseconds (10 to 500 ms).
    pub fn set_grain_size(&mut self, ms: f64) {
        self.grain_size_ms = ms.clamp(MIN_TEXTURE_GRAIN_MS, MAX_TEXTURE_GRAIN_MS);
    }

    /// Sets Flux percentage (0 to 100 %).
    pub fn set_flux(&mut self, flux: f64) {
        self.flux = flux.clamp(MIN_TEXTURE_FLUX, MAX_TEXTURE_FLUX);
    }

    /// Sets the clip the grains' randomness is seeded from (R6), reseeding
    /// and starting afresh where it changed.
    pub fn set_clip_id(&mut self, clip_id: u64) {
        if self.seed != clip_id {
            self.seed = clip_id;
            self.random = Random::seeded(clip_id);
            self.reset();
        }
    }

    /// Computes the next grain's length in frames and its file position jitter,
    /// according to Grain Size and Flux (for tests).
    pub fn grain_length_and_jitter(&mut self) -> (usize, f64) {
        let g_max = (MAX_TEXTURE_GRAIN_MS / 1000.0 * self.sample_rate).ceil() as usize;
        let g_nominal = ((self.grain_size_ms * self.sample_rate / 1000.0).round() as usize)
            .clamp(16, g_max.max(16));
        if self.flux <= 0.0 {
            (g_nominal, 0.0)
        } else {
            let r_len = self.random.next_bipolar();
            let r_pos = self.random.next_bipolar();
            let max_jitter = (self.flux / 100.0) * g_nominal as f64;
            let len =
                ((g_nominal as f64 + r_len * max_jitter).round() as usize).clamp(16, 2 * g_nominal);
            let pos_jitter = r_pos * max_jitter;
            (len, pos_jitter)
        }
    }

    /// Primes initial overlapping grains anchored at song frame `start`.
    fn prime_at(&mut self, start: i64, request: &Request<'_>, window: &Window<'_>) {
        self.next_grain_start = start;
        self.spawn_grain(window, request, request.positions[0]);
    }

    /// Spawns the next grain with Flux jitter.
    fn spawn_grain(&mut self, window: &Window<'_>, request: &Request<'_>, pos_at_grain: f64) {
        let (len, pos_jitter) = self.grain_length_and_jitter();
        let hop = (len / 2).max(1);

        let grain_t = self.next_grain_start;
        let start = request.first;
        let end = start + request.positions.len() as i64;
        let base_pos = if grain_t >= start && grain_t < end {
            request.positions[(grain_t - start) as usize]
        } else {
            pos_at_grain + (grain_t - start) as f64 * request.speed
        };
        let nominal_pos = base_pos + pos_jitter;

        let detected = self
            .periods
            .period(window, nominal_pos, self.sample_rate, hop);

        let pitch_ratio = request.pitch.max(0.01);
        let out_len = ((len as f64) / pitch_ratio).round() as usize;
        let out_hop = (((hop as f64) / pitch_ratio).round() as usize).max(1);

        // Spec: warp-modes R19. Where the audio has a clear pitch, a grain
        // starts on the period nearest where the warp map reads, keeping the
        // overlapping grains in phase so a steady tone keeps its pitch; where
        // it hasn't, it starts where the map reads, Flux's jitter included.
        let file_pos = if let (Some(p), Some((last_t, last_pos))) = (detected, self.last_grain) {
            let p_prev = last_pos + (grain_t - last_t) as f64 * pitch_ratio;
            let k = ((nominal_pos - p_prev) / p).round();
            p_prev + k * p
        } else {
            nominal_pos
        };
        self.last_grain = Some((grain_t, file_pos));

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
    }
}

impl WarpMode for Texture {
    fn reach(&self, request: &Request<'_>) -> (usize, usize) {
        if request.neutral {
            return (0, 0);
        }
        let g = (self.grain_size_ms * self.sample_rate / 1000.0).ceil() as usize;
        let jitter = ((self.flux / 100.0) * g as f64).ceil() as usize;
        let interp = Interpolation::of(request.high_quality)
            .reach(request.speed)
            .0;
        let reach = g * 2 + jitter * 2 + interp + 64;
        (reach, reach)
    }

    // Spec: warp-modes R19, R20
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
        self.last_grain = None;
        self.periods = PeriodCache::default();
        self.random = Random::seeded(self.seed);
    }
}
