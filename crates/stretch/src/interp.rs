//! The interpolators a clip is read through wherever it plays at a rate other
//! than 1 (the warp modes spec, R12): HiQ on reads through a long
//! band-limited windowed sinc, off through a short one.
//!
//! Both read a window of file frames around the position, which the caller
//! has made ready ([`Interpolation::reach`] says how far each way), and give
//! the frame at the position's fractional place between two of them. At a
//! speed of 1, a position on a whole frame is that frame, unchanged, so a
//! clip at rate 1 is not touched. At any other speed every output frame goes
//! through the kernel, a whole-frame position too: a clip read twice as fast
//! lands on whole frames only, and would otherwise skip the filter that keeps
//! what the output can't carry out of it.
//!
//! The kernels are Kaiser-windowed sincs, and their numbers are chosen against
//! R36's targets (WP-4.19). A kernel's *cutoff* is the middle of its
//! transition from pass to stop, in fractions of the sampling rate, and the
//! transition is `(attenuation - 7.95) / (14.36 * taps)` wide (Kaiser, 1974),
//! so each is placed to pass what its target asks to be flat and to stop
//! everything from the Nyquist frequency up:
//!
//! - Long (HiQ on): 128 taps and 90 dB, cutoff 0.475: passes to 0.4526, which
//!   is past 90 % of the band, and stops from 0.4974.
//! - Short (HiQ off): 28 taps and 60 dB, cutoff 0.425: passes to 0.36, past
//!   70 % of the band, and stops from 0.49.

use std::sync::OnceLock;

/// How far a speed above 1 widens a kernel (to keep the cutoff below the
/// output's Nyquist frequency): up to sixteen times, which is 48 semitones, the
/// most a clip is transposed (R25); a faster clip is read through the
/// sixteen-times kernel.
const SINC_WIDEN: f64 = 16.0;
/// Kernel table entries per input frame.
const SINC_RESOLUTION: usize = 512;
/// The long kernel's reach in input frames each side of the position at rate 1.
const LONG_HALF: usize = 64;
/// The most taps any kernel has: both sides of the long kernel at its widest.
const MAX_TAPS: usize = 2 * LONG_HALF * SINC_WIDEN as usize;

/// One kernel's shape: its reach in input frames each side of the position at
/// rate 1, the Kaiser window's β, and its cutoff in fractions of the rate;
/// and its table, made the first time it is wanted.
struct Shape {
    half: usize,
    beta: f64,
    cutoff: f64,
    table: OnceLock<Vec<f32>>,
}

/// HiQ on: about 90 dB of attenuation.
static LONG: Shape = Shape {
    half: LONG_HALF,
    beta: 9.0,
    cutoff: 0.475,
    table: OnceLock::new(),
};
/// HiQ off: about 60 dB.
static SHORT: Shape = Shape {
    half: 14,
    beta: 5.65,
    cutoff: 0.425,
    table: OnceLock::new(),
};

/// Which interpolator reads the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// A short windowed sinc: HiQ off.
    Short,
    /// A long windowed sinc, with its cutoff held below the output's Nyquist
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
            Self::Short
        }
    }

    /// Prepares the kernels the interpolators read through, which takes a
    /// moment the first time: call it off the audio thread, before an
    /// interpolator is first used.
    pub fn warm() {
        let _ = (kernel(&LONG), kernel(&SHORT));
    }

    /// How many file frames the interpolator reads before and after the
    /// frame a position falls in, at `speed` file frames per output frame.
    /// The speed is negative for a backward read; its magnitude sets how far
    /// the kernel widens.
    #[must_use]
    pub fn reach(self, speed: f64) -> (usize, usize) {
        let half = half(self.shape(), speed);
        (half - 1, half)
    }

    /// The frame at `position + fraction` (0 to under 1) of `channel`, from
    /// `window`, whose frame `at` is the one `position` falls in, at `speed`
    /// (negative for a backward read, which widens the kernel by its
    /// magnitude). The window must hold the frames [`reach`](Self::reach) says.
    #[must_use]
    pub fn sample(self, window: &[f32], at: usize, fraction: f64, speed: f64) -> f32 {
        if fraction == 0.0 && speed == 1.0 {
            return window[at];
        }
        let mut taps = Taps::new();
        self.weights(at, fraction, speed, &mut taps);
        taps.dot(window)
    }

    /// Works out the weights [`sample`](Self::sample) reads one position
    /// through, once for every channel: [`Taps::dot`] applies them to a
    /// channel's window, which must hold the frames [`reach`](Self::reach)
    /// says.
    pub(crate) fn weights(self, at: usize, fraction: f64, speed: f64, taps: &mut Taps) {
        if fraction == 0.0 && speed == 1.0 {
            // Spec: warp-modes R4. The frame itself, untouched.
            (taps.first, taps.len, taps.sum, taps.weights[0]) = (at, 1, 1.0, 1.0);
        } else {
            fill(self.shape(), at, fraction, speed, taps);
        }
    }

    fn shape(self) -> &'static Shape {
        match self {
            Self::Short => &SHORT,
            Self::Sinc => &LONG,
        }
    }
}

/// A kernel's half width in frames at `speed`.
fn half(shape: &Shape, speed: f64) -> usize {
    (shape.half as f64 * speed.abs().clamp(1.0, SINC_WIDEN)).ceil() as usize
}

/// The weights of one position's taps: `len` frames from `first`, scaled by
/// `sum` so a steady level passes unchanged.
pub(crate) struct Taps {
    weights: [f64; MAX_TAPS],
    first: usize,
    len: usize,
    sum: f64,
}

impl Taps {
    pub(crate) fn new() -> Self {
        Self {
            weights: [0.0; MAX_TAPS],
            first: 0,
            len: 0,
            sum: 1.0,
        }
    }

    /// The taps' weighted sum over `window`.
    pub(crate) fn dot(&self, window: &[f32]) -> f32 {
        let frames = &window[self.first..self.first + self.len];
        let mut sum = 0.0_f64;
        for (&sample, &weight) in frames.iter().zip(&self.weights) {
            sum += f64::from(sample) * weight;
        }
        (sum / self.sum) as f32
    }
}

#[cfg(test)]
thread_local! {
    /// How many positions' weights this thread has worked out.
    pub(crate) static KERNEL_EVALS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The windowed sinc's weights at `position + fraction`.
fn fill(shape: &Shape, at: usize, fraction: f64, speed: f64, taps: &mut Taps) {
    #[cfg(test)]
    KERNEL_EVALS.with(|evals| evals.set(evals.get() + 1));
    let widen = speed.abs().clamp(1.0, SINC_WIDEN);
    let half = half(shape, speed);
    let table = kernel(shape);
    // The kernel is the prototype stretched by `widen` in frames: a tap `k`
    // frames from the position sits `(k - fraction) / widen` along it.
    let step = SINC_RESOLUTION as f64 / widen;
    let first = at + 1 - half;
    let mut sum = 0.0_f64;
    for (offset, slot) in taps.weights[..2 * half].iter_mut().enumerate() {
        // The tap's distance from the position, in input frames.
        let distance = ((first + offset) as f64 - at as f64 - fraction).abs() * step;
        let i = distance as usize;
        // Beyond the kernel's reach the weight is nothing, so the taps that
        // count are symmetric about the position and a ramp reads exactly.
        let weight = if i < shape.half * SINC_RESOLUTION {
            let along = distance - i as f64;
            f64::from(table[i]) * (1.0 - along) + f64::from(table[i + 1]) * along
        } else {
            0.0
        };
        *slot = weight;
        sum += weight;
    }
    (taps.first, taps.len, taps.sum) = (first, 2 * half, sum);
}

/// The prototype kernel of `shape`: a Kaiser-windowed sinc with its cutoff at
/// the shape's, one frame wide per unit, sampled at [`SINC_RESOLUTION`]
/// entries per frame, from 0 to its reach.
fn kernel(shape: &Shape) -> &[f32] {
    shape.table.get_or_init(|| {
        let reach = shape.half;
        let denominator = bessel_i0(shape.beta);
        (0..=reach * SINC_RESOLUTION + 1)
            .map(|i| {
                let x = i as f64 / SINC_RESOLUTION as f64;
                let sinc = if x == 0.0 {
                    1.0
                } else {
                    let t = 2.0 * shape.cutoff * x * std::f64::consts::PI;
                    t.sin() / t
                };
                let along = (x / reach as f64).min(1.0);
                let window = bessel_i0(shape.beta * (1.0 - along * along).sqrt()) / denominator;
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
        let window = sine(0.1, 400);
        for kind in [Interpolation::Short, Interpolation::Sinc] {
            assert_eq!(kind.sample(&window, 200, 0.0, 1.0), window[200]);
        }
    }

    // Spec: warp-modes R12. A clip read twice as fast lands on whole frames,
    // and they go through the filter all the same.
    #[test]
    fn a_whole_frame_is_filtered_at_any_other_speed() {
        // A tone of 0.4 of the rate, which a read at twice the speed can't
        // carry: the filter takes it out where a bare read would keep it.
        let window = sine(0.4, 1_200);
        for kind in [Interpolation::Short, Interpolation::Sinc] {
            let got = kind.sample(&window, 600, 0.0, 2.0);
            assert!(got.abs() < 1e-3, "{kind:?}: {got}");
        }
    }

    // Spec: warp-modes R12, R36. A Back-and-Forth Beats pass reads backwards
    // (slope -ratio); its filter must widen just as a forward read's does.
    #[test]
    fn a_backward_read_is_filtered_like_a_forward_one() {
        let window = sine(0.4, 1_200);
        for kind in [Interpolation::Short, Interpolation::Sinc] {
            assert_eq!(kind.reach(-2.0), kind.reach(2.0), "{kind:?}");
            let got = kind.sample(&window, 600, 0.0, -2.0);
            assert!(got.abs() < 1e-3, "{kind:?}: {got}");
        }
    }

    #[test]
    fn both_interpolators_find_a_slow_sine_between_its_frames() {
        let window = sine(0.02, 400);
        for kind in [Interpolation::Short, Interpolation::Sinc] {
            let got = kind.sample(&window, 200, 0.37, 1.0);
            let want = (2.0 * std::f64::consts::PI * 0.02 * 200.37).sin() as f32;
            assert!((got - want).abs() < 2e-4, "{kind:?}: {got} {want}");
        }
    }

    // Spec: warp-modes R12. The long kernel passes a tone the short one has
    // begun to take down.
    #[test]
    fn the_long_kernel_follows_a_high_tone_closer_than_the_short_one() {
        let window = sine(0.44, 400);
        let want = (2.0 * std::f64::consts::PI * 0.44 * 200.5).sin() as f32;
        let error = |kind: Interpolation| (kind.sample(&window, 200, 0.5, 1.0) - want).abs();
        assert!(error(Interpolation::Sinc) < 1e-3);
        assert!(error(Interpolation::Sinc) < error(Interpolation::Short) / 10.0);
    }

    #[test]
    fn reading_faster_widens_the_kernel_and_keeps_a_steady_level() {
        let (before, after) = Interpolation::Sinc.reach(1.0);
        assert_eq!((before, after), (LONG.half - 1, LONG.half));
        assert!(Interpolation::Sinc.reach(2.0).1 > after);
        assert_eq!(Interpolation::Sinc.reach(100.0).1, LONG.half * 16);
        let steady = vec![0.5_f32; 1_200];
        let level = Interpolation::Sinc.sample(&steady, 600, 0.3, 2.0);
        assert!((level - 0.5).abs() < 1e-6);
    }
}
