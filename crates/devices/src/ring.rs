//! A ring of recent samples read at a fractional delay through a windowed
//! sinc (PitchLoop89 and Vector Delay): a read head that moves, as these
//! devices' do, must interpolate well or it aliases.

use core::f64::consts::PI;

use crate::multisampler::bessel_i0;

/// Taps in the kernel, half before the read place and half after it.
const TAPS: usize = 16;
/// How finely the kernel's table divides the gap between two frames.
const PHASES: usize = 256;
/// The shortest delay a read takes: the kernel needs the frames after the
/// read place to be written already.
pub(crate) const MIN_DELAY: f64 = (TAPS / 2 + 2) as f64;

/// Where a fed-back sample starts to be limited, so a loop that adds up to
/// more than one settles instead of growing.
const KNEE: f32 = 0.9;

/// A fed-back sample, held to ±1 by a soft knee.
pub(crate) fn limit(x: f32) -> f32 {
    let over = x.abs() - KNEE;
    if over <= 0.0 {
        x
    } else {
        x.signum() * (KNEE + (1.0 - KNEE) * (over / (1.0 - KNEE)).tanh())
    }
}

/// The Kaiser-windowed sinc a [`Ring`] reads through, at [`PHASES`]
/// fractional places, each row summing to one.
pub(crate) struct Kernel {
    table: Vec<f32>,
}

impl Kernel {
    pub(crate) fn new() -> Self {
        let beta = 9.0f64;
        let norm = bessel_i0(beta);
        let half = (TAPS / 2) as f64;
        let mut table = vec![0.0f32; TAPS * (PHASES + 1)];
        for phase in 0..=PHASES {
            let frac = phase as f64 / PHASES as f64;
            let mut row = [0.0f64; TAPS];
            for (tap, value) in row.iter_mut().enumerate() {
                // The tap reads the frame (tap − half + 1) from the floor
                // frame, so it lies that far from the read place less the
                // fraction.
                let x = tap as f64 - half + 1.0 - frac;
                let sinc = if x.abs() < 1e-12 {
                    1.0
                } else {
                    (PI * x).sin() / (PI * x)
                };
                let t = x / half;
                let window = if t.abs() >= 1.0 {
                    0.0
                } else {
                    bessel_i0(beta * (1.0 - t * t).sqrt()) / norm
                };
                *value = sinc * window;
            }
            let sum: f64 = row.iter().sum();
            for (tap, value) in row.iter().enumerate() {
                table[phase * TAPS + tap] = (value / sum) as f32;
            }
        }
        Self { table }
    }
}

/// The last `len` samples written.
pub(crate) struct Ring {
    buffer: Vec<f32>,
    write: usize,
}

impl Ring {
    pub(crate) fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len.max(2 * TAPS)],
            write: 0,
        }
    }

    /// The longest delay a read takes, in frames.
    pub(crate) fn max_delay(&self) -> f64 {
        (self.buffer.len() - TAPS) as f64
    }

    /// Adds `sample` and moves on.
    pub(crate) fn write(&mut self, sample: f32) {
        self.buffer[self.write] = sample;
        self.write = (self.write + 1) % self.buffer.len();
    }

    /// The signal `delay` frames before the next one to be written, from
    /// [`MIN_DELAY`] to [`max_delay`](Self::max_delay) (clamped to them),
    /// at any fraction of a frame.
    pub(crate) fn read(&self, kernel: &Kernel, delay: f64) -> f32 {
        let len = self.buffer.len();
        let delay = delay.clamp(MIN_DELAY, self.max_delay());
        let place = (self.write as f64 - delay).rem_euclid(len as f64);
        let floor = place.floor();
        let scaled = (place - floor) * PHASES as f64;
        let phase = (scaled as usize).min(PHASES - 1);
        let blend = (scaled - phase as f64) as f32;
        let a = &kernel.table[phase * TAPS..(phase + 1) * TAPS];
        let b = &kernel.table[(phase + 1) * TAPS..(phase + 2) * TAPS];
        let first = floor as usize + len - (TAPS / 2 - 1);
        let mut sum = 0.0f32;
        for tap in 0..TAPS {
            sum += self.buffer[(first + tap) % len] * (a[tap] + (b[tap] - a[tap]) * blend);
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: vector-delay R4
    #[test]
    fn a_read_place_sliding_at_a_steady_rate_adds_no_alias_above_minus_70_db() {
        let (hz, rate, len) = (2_000.0f64, 48_000.0f64, 24_000);
        let kernel = Kernel::new();
        let mut ring = Ring::new(len);
        for n in 0..len {
            ring.write((core::f64::consts::TAU * hz * n as f64 / rate).sin() as f32);
        }
        let capacity = ring.max_delay();
        for speed in [0.7f64, 1.3, 2.0] {
            // The read place moves on `speed` frames a frame through the
            // frozen ring, so a tone of `hz` comes out at `speed × hz`.
            let frames = ((capacity - MIN_DELAY) / speed) as usize - 8;
            let out: Vec<f64> = (0..frames)
                .map(|m| f64::from(ring.read(&kernel, capacity - speed * m as f64)))
                .collect();
            // Fit the sine of the expected frequency; what's left is alias.
            let w = core::f64::consts::TAU * speed * hz / rate;
            let (mut sin_sum, mut cos_sum) = (0.0, 0.0);
            for (m, v) in out.iter().enumerate() {
                sin_sum += v * (w * m as f64).sin();
                cos_sum += v * (w * m as f64).cos();
            }
            let scale = 2.0 / frames as f64;
            let (a, b) = (sin_sum * scale, cos_sum * scale);
            let (mut rest, mut tone) = (0.0, 0.0);
            for (m, v) in out.iter().enumerate() {
                let fit = a * (w * m as f64).sin() + b * (w * m as f64).cos();
                rest += (v - fit).powi(2);
                tone += fit.powi(2);
            }
            let db = 10.0 * (rest / tone).log10();
            assert!(
                db < -70.0,
                "at {speed}: the alias is {db} dB below the tone"
            );
        }
    }
}
