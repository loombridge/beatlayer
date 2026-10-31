//! Granular time-stretching building blocks: grains, windowing, and resampling.
//!
//! Grains are placed along the synthesis timeline, sampled from the file
//! through HiQ's interpolator (the warp modes spec, R20), windowed with a Hann
//! window, and summed. Normalizing by the overlapping window weights ensures
//! flat unity gain and no amplitude ripple even with varying grain sizes or
//! Flux jitter.

use crate::interp::{Interpolation, Taps};
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

/// Writes song frame `t`'s frame to the start of `out` (planar, `stride` apart):
/// every grain sounding on it, overlap-added and divided by the windows'
/// weights there, so the level stays flat whatever the grains' lengths or
/// places (R18, R19, R20). A grain is read from `window` at its own pitch
/// ratio, through `interpolation`.
pub(crate) fn render_frame(
    grains: &ActiveGrains,
    window: &Window<'_>,
    interpolation: Interpolation,
    taps: &mut Taps,
    t: i64,
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
            // A grain outside the window is silence, in every channel.
            let floor = pos.floor();
            let at = floor as i64 - window.first;
            let (before, after) = interpolation.reach(grain.pitch_ratio);
            if at >= before as i64 && at + (after as i64) < window.frames as i64 {
                interpolation.weights(at as usize, pos - floor, grain.pitch_ratio, taps);
                for (c, slot) in sum_ch.iter_mut().enumerate().take(channels) {
                    *slot += w * taps.dot(window.channel(c));
                }
            }
            weight_sum += w;
        }
    }
    for (c, &sum) in sum_ch.iter().enumerate().take(channels) {
        out[c * stride] = if weight_sum > 1e-6 {
            sum / weight_sum
        } else {
            sum
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two different channels: a sine, and a ramp.
    fn stereo(frames: usize) -> Vec<f32> {
        let left = (0..frames).map(|i| (i as f32 * 0.0123 * std::f32::consts::TAU).sin());
        let right = (0..frames).map(|i| i as f32 / frames as f32);
        left.chain(right).collect()
    }

    fn grains() -> ActiveGrains {
        let mut grains = ActiveGrains::default();
        for (start, pitch_ratio) in [(0, 1.5), (40, 0.5)] {
            grains.push(Grain {
                start,
                len: 200,
                file_pos: 1_000.3,
                pitch_ratio,
            });
        }
        grains
    }

    // Spec: warp-modes R18 to R20. Each channel is the overlap-add of the
    // grains the interpolator reads from it alone.
    #[test]
    fn a_frame_is_the_overlap_add_of_each_channel_read_alone() {
        let data = stereo(4_000);
        let window = Window {
            data: &data,
            stride: 4_000,
            channels: 2,
            frames: 4_000,
            first: 0,
        };
        let grains = grains();
        for interpolation in [Interpolation::Short, Interpolation::Sinc] {
            for t in [10, 60, 150] {
                let mut out = [0.0; 2];
                let mut taps = Taps::new();
                render_frame(&grains, &window, interpolation, &mut taps, t, &mut out, 1);
                for (c, &got) in out.iter().enumerate() {
                    let (mut sum, mut weights) = (0.0_f32, 0.0_f32);
                    for grain in grains.iter().filter(|g| t >= g.start) {
                        let m = (t - grain.start) as usize;
                        let w = hann_window(m, grain.len);
                        let pos = grain.file_pos + m as f64 * grain.pitch_ratio;
                        let at = pos.floor();
                        sum += w * interpolation.sample(
                            window.channel(c),
                            at as usize,
                            pos - at,
                            grain.pitch_ratio,
                        );
                        weights += w;
                    }
                    assert_eq!(got.to_bits(), (sum / weights).to_bits());
                }
            }
        }
    }

    // Spec: warp-modes R82
    #[test]
    fn a_grain_computes_kernel_weights_once_for_every_channel() {
        let data = stereo(4_000);
        let window = Window {
            data: &data,
            stride: 4_000,
            channels: 2,
            frames: 4_000,
            first: 0,
        };
        let mut grains = ActiveGrains::default();
        grains.push(Grain {
            start: 0,
            len: 200,
            file_pos: 1_000.3,
            pitch_ratio: 1.5,
        });
        let mut out = [0.0; 2];
        crate::interp::KERNEL_EVALS.with(|evals| evals.set(0));
        let mut taps = Taps::new();
        render_frame(
            &grains,
            &window,
            Interpolation::Sinc,
            &mut taps,
            10,
            &mut out,
            1,
        );
        assert_eq!(crate::interp::KERNEL_EVALS.with(std::cell::Cell::get), 1);
    }
}
