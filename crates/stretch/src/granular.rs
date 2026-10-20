//! Granular time-stretching building blocks: grains, windowing, and resampling.
//!
//! Grains are placed along the synthesis timeline, sampled from the file
//! through HiQ's interpolator (the warp modes spec, R20), windowed with a Hann
//! window, and summed. Normalizing by the overlapping window weights ensures
//! flat unity gain and no amplitude ripple even with varying grain sizes or
//! Flux jitter.

use crate::interp::Interpolation;
use crate::mode::Window;

/// One grain on the synthesis timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grain {
    /// Output (song) frame where this grain begins.
    pub start: i64,
    /// Output length of this grain in frames.
    pub len: usize,
    /// File frame the grain begins reading from.
    pub file_pos: f64,
    /// Pitch ratio used for resampling this grain (R20).
    pub pitch_ratio: f64,
}

/// Symmetrically centered Hann window, non-zero at endpoints.
#[inline]
pub fn hann_window(m: usize, len: usize) -> f32 {
    if len <= 1 {
        return 1.0;
    }
    let phase = std::f64::consts::TAU * (m as f64 + 0.5) / len as f64;
    0.5 * (1.0 - phase.cos() as f32)
}

/// Samples `channel` at `pos` through `interpolation`, or silence if outside.
#[inline]
pub fn sample_window(
    channel: &[f32],
    window_first: i64,
    pos: f64,
    interpolation: Interpolation,
    speed: f64,
) -> f32 {
    let floor = pos.floor();
    let at_i64 = floor as i64 - window_first;
    let (reach_before, reach_after) = interpolation.reach(speed);
    if at_i64 < reach_before as i64 || at_i64 + reach_after as i64 >= channel.len() as i64 {
        return 0.0;
    }
    let at = at_i64 as usize;
    let fraction = pos - floor;
    interpolation.sample(channel, at, fraction, speed)
}

const MAX_ACTIVE_GRAINS: usize = 8;

/// A fixed-capacity set of currently active grains (no audio-thread allocation).
#[derive(Clone, Copy, Debug)]
pub struct ActiveGrains {
    grains: [Option<Grain>; MAX_ACTIVE_GRAINS],
}

impl Default for ActiveGrains {
    fn default() -> Self {
        Self {
            grains: [None; MAX_ACTIVE_GRAINS],
        }
    }
}

impl ActiveGrains {
    /// Clears all active grains.
    pub fn clear(&mut self) {
        self.grains = [None; MAX_ACTIVE_GRAINS];
    }

    /// Adds a grain to the first open slot, or replaces the oldest grain if full.
    pub fn push(&mut self, grain: Grain) {
        for slot in &mut self.grains {
            if slot.is_none() {
                *slot = Some(grain);
                return;
            }
        }
        let mut oldest = 0;
        let mut min_start = i64::MAX;
        for (i, g) in self.grains.iter().enumerate() {
            if g.as_ref().is_some_and(|g| g.start < min_start) {
                min_start = g.as_ref().unwrap().start;
                oldest = i;
            }
        }
        self.grains[oldest] = Some(grain);
    }

    /// Retires grains whose output span has passed `current_frame`.
    pub fn prune(&mut self, current_frame: i64) {
        for slot in &mut self.grains {
            if slot
                .as_ref()
                .is_some_and(|g| current_frame >= g.start + g.len as i64)
            {
                *slot = None;
            }
        }
    }

    /// Iterates over currently active grains.
    pub fn iter(&self) -> impl Iterator<Item = &Grain> {
        self.grains.iter().filter_map(|g| g.as_ref())
    }
}

/// The most channels a grain is summed into.
const CHANNELS: usize = 8;

/// Writes song frame `t`'s frame to `out` (planar, `stride` apart):
/// every grain sounding on it, overlap-added and divided by the windows'
/// weights there, so the level stays flat whatever the grains' lengths or
/// places (R18, R19, R20). A grain is read from `window` at its own pitch
/// ratio, through `interpolation`.
pub fn render_frame(
    grains: &ActiveGrains,
    window: &Window<'_>,
    interpolation: Interpolation,
    t: i64,
    i: usize,
    out: &mut [f32],
    stride: usize,
) {
    let channels = window.channels.min(CHANNELS);
    let mut sum_ch = [0.0f32; CHANNELS];
    let mut weight_sum = 0.0f32;
    for grain in grains.iter() {
        if t >= grain.start && t < grain.start + grain.len as i64 {
            let m = (t - grain.start) as usize;
            let w = hann_window(m, grain.len);
            let pos = grain.file_pos + m as f64 * grain.pitch_ratio;
            for (c, slot) in sum_ch.iter_mut().enumerate().take(channels) {
                *slot += w * sample_window(
                    window.channel(c),
                    window.first,
                    pos,
                    interpolation,
                    grain.pitch_ratio,
                );
            }
            weight_sum += w;
        }
    }
    for (c, &sum) in sum_ch.iter().enumerate().take(channels) {
        out[c * stride + i] = if weight_sum > 1e-6 {
            sum / weight_sum
        } else {
            sum
        };
    }
}
