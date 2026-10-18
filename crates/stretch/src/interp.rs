//! The interpolators a clip is read through wherever it plays at a rate other
//! than 1 (the warp modes spec, R12): HiQ on reads through a band-limited
//! windowed sinc, off through a cubic.
//!
//! Both read a window of file frames around the position, which the caller
//! has made ready ([`Interpolation::reach`] says how far each way), and give
//! the frame at the position's fractional place between two of them. A
//! position on a whole frame is that frame, unchanged, so a clip at rate 1
//! is not touched.

use std::sync::OnceLock;

/// The sinc kernel's reach, in input frames each side of the position, at
/// rate 1.
const SINC_HALF: usize = 32;
/// How far a speed above 1 widens it (to keep the cutoff below the output's
/// Nyquist frequency): up to eight times; a faster clip is read through the
/// eight-times kernel.
const SINC_WIDEN: f64 = 8.0;
/// Kernel table entries per input frame.
const SINC_RESOLUTION: usize = 512;
/// The Kaiser window's shape, for about 90 dB of attenuation.
const KAISER_BETA: f64 = 9.0;

/// Which interpolator reads the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// Four-point cubic: HiQ off.
    Cubic,
    /// Windowed sinc, with its cutoff held below the output's Nyquist
    /// frequency when it reads faster than the file's rate: HiQ on.
    Sinc,
}

impl Interpolation {
    /// The interpolator HiQ chooses.
    #[must_use]
    pub fn of(high_quality: bool) -> Self {
        if high_quality {
            Self::Sinc
        } else {
            Self::Cubic
        }
    }

    /// Prepares the kernel the sinc reads through, which takes a moment the
    /// first time: call it off the audio thread, before the interpolator is
    /// first used.
    pub fn warm() {
        let _ = kernel();
    }

    /// How many file frames the interpolator reads before and after the
    /// frame a position falls in, at `speed` file frames per output frame.
    #[must_use]
    pub fn reach(self, speed: f64) -> (usize, usize) {
        match self {
            Self::Cubic => (1, 2),
            Self::Sinc => {
                let half = sinc_half(speed);
                (half - 1, half)
            }
        }
    }

    /// The frame at `position + fraction` (0 to under 1) of `channel`, from
    /// `window`, whose frame `at` is the one `position` falls in, at `speed`.
    /// The window must hold the frames [`reach`](Self::reach) says.
    #[must_use]
    pub fn sample(self, window: &[f32], at: usize, fraction: f64, speed: f64) -> f32 {
        if fraction == 0.0 {
            return window[at];
        }
        match self {
            Self::Cubic => cubic(&window[at - 1..at + 3], fraction),
            Self::Sinc => sinc(window, at, fraction, speed),
        }
    }
}

/// The kernel's half width in frames at `speed`.
fn sinc_half(speed: f64) -> usize {
    (SINC_HALF as f64 * speed.clamp(1.0, SINC_WIDEN)).ceil() as usize
}

/// Catmull-Rom through the four frames `p`, at `t` between `p[1]` and `p[2]`.
fn cubic(p: &[f32], t: f64) -> f32 {
    let (a, b, c, d) = (
        f64::from(p[0]),
        f64::from(p[1]),
        f64::from(p[2]),
        f64::from(p[3]),
    );
    let m1 = 0.5 * (c - a);
    let m2 = 0.5 * (d - b);
    let (t2, t3) = (t * t, t * t * t);
    ((2.0 * t3 - 3.0 * t2 + 1.0) * b
        + (t3 - 2.0 * t2 + t) * m1
        + (-2.0 * t3 + 3.0 * t2) * c
        + (t3 - t2) * m2) as f32
}

/// The windowed sinc at `position + fraction`, its weights scaled to sum to 1
/// so a steady level passes unchanged.
fn sinc(window: &[f32], at: usize, fraction: f64, speed: f64) -> f32 {
    let widen = speed.clamp(1.0, SINC_WIDEN);
    let half = sinc_half(speed);
    let table = kernel();
    // The kernel is the prototype stretched by `widen` in frames: a tap `k`
    // frames from the position sits `(k - fraction) / widen` along it.
    let step = SINC_RESOLUTION as f64 / widen;
    let (mut sum, mut weights) = (0.0_f64, 0.0_f64);
    let first = at + 1 - half;
    for (offset, &sample) in window[first..=at + half].iter().enumerate() {
        // The tap's distance from the position, in input frames.
        let distance = ((first + offset) as f64 - at as f64 - fraction).abs() * step;
        let i = distance as usize;
        let weight = if i + 1 < table.len() {
            let along = distance - i as f64;
            f64::from(table[i]) * (1.0 - along) + f64::from(table[i + 1]) * along
        } else {
            0.0
        };
        sum += f64::from(sample) * weight;
        weights += weight;
    }
    (sum / weights) as f32
}

/// The prototype kernel: a Kaiser-windowed sinc with its cutoff at the
/// Nyquist frequency, one frame wide per unit, sampled at
/// [`SINC_RESOLUTION`] entries per frame, from 0 to its reach.
fn kernel() -> &'static [f32] {
    static KERNEL: OnceLock<Vec<f32>> = OnceLock::new();
    KERNEL.get_or_init(|| {
        let reach = SINC_HALF;
        let denominator = bessel_i0(KAISER_BETA);
        (0..=reach * SINC_RESOLUTION + 1)
            .map(|i| {
                let x = i as f64 / SINC_RESOLUTION as f64;
                let sinc = if x == 0.0 {
                    1.0
                } else {
                    (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                };
                let along = (x / reach as f64).min(1.0);
                let window = bessel_i0(KAISER_BETA * (1.0 - along * along).sqrt()) / denominator;
                (sinc * window) as f32
            })
            .collect()
    })
}

/// The modified Bessel function of the first kind, order 0, by its series.
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..60 {
        term *= (half / f64::from(k)).powi(2);
        sum += term;
        if term < sum * 1e-18 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency: f64, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| (2.0 * std::f64::consts::PI * frequency * i as f64).sin() as f32)
            .collect()
    }

    // Spec: warp-modes R12
    #[test]
    fn a_whole_frame_is_returned_unchanged() {
        let window = sine(0.1, 200);
        for kind in [Interpolation::Cubic, Interpolation::Sinc] {
            assert_eq!(kind.sample(&window, 100, 0.0, 1.0), window[100]);
        }
    }

    #[test]
    fn both_interpolators_find_a_slow_sine_between_its_frames() {
        let window = sine(0.02, 400);
        for kind in [Interpolation::Cubic, Interpolation::Sinc] {
            let got = kind.sample(&window, 200, 0.37, 1.0);
            let want = (2.0 * std::f64::consts::PI * 0.02 * 200.37).sin() as f32;
            assert!((got - want).abs() < 2e-4, "{kind:?}: {got} {want}");
        }
    }

    // Spec: warp-modes R12. The sinc is the better of the two near the top
    // of the band.
    #[test]
    fn the_sinc_follows_a_high_tone_closer_than_the_cubic() {
        let window = sine(0.3, 400);
        let want = (2.0 * std::f64::consts::PI * 0.3 * 200.5).sin() as f32;
        let error = |kind: Interpolation| (kind.sample(&window, 200, 0.5, 1.0) - want).abs();
        assert!(error(Interpolation::Sinc) < 1e-3);
        assert!(error(Interpolation::Sinc) < error(Interpolation::Cubic) / 10.0);
    }

    #[test]
    fn reading_faster_widens_the_kernel_and_keeps_a_steady_level() {
        let (before, after) = Interpolation::Sinc.reach(1.0);
        assert_eq!((before, after), (SINC_HALF - 1, SINC_HALF));
        assert!(Interpolation::Sinc.reach(2.0).1 > after);
        assert_eq!(Interpolation::Sinc.reach(100.0).1, SINC_HALF * 8);
        let steady = vec![0.5_f32; 600];
        let level = Interpolation::Sinc.sample(&steady, 300, 0.3, 2.0);
        assert!((level - 0.5).abs() < 1e-6);
    }
}
