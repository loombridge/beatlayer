//! A complex FFT in the four-step form, run a few small transforms at a time.
//!
//! A transform of `r * c` points is `c` transforms of `r` points over the
//! columns of an `r` by `c` matrix, a twiddle multiplication, then `r`
//! transforms of `c` points over its rows. Each of those small transforms is
//! one *step*, so a caller can spread the whole transform over as many
//! blocks as it likes and the work in each block is the same: that is what
//! keeps the convolver's load flat (Spec: the architecture's `bl-dsp` row).
//!
//! The matrix is read and written through closures, so the caller chooses
//! where the input comes from and where the output goes, and the output
//! comes out in natural order.

use std::f64::consts::TAU;
use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

/// One four-step transform's plans and tables, shared by every job of its size.
pub struct FourStep {
    r: usize,
    c: usize,
    /// `e^(-2 pi i j / (r c))` for `j` below `r * c`.
    twiddle: Vec<Complex32>,
    /// Forward over `r` points, forward over `c`, inverse over `r`, inverse over `c`.
    plans: [Arc<dyn Fft<f32>>; 4],
    scratch: Vec<Complex32>,
    line: Vec<Complex32>,
    calls: u64,
}

impl FourStep {
    /// A transform of `r * c` points. Allocates.
    pub fn new(r: usize, c: usize) -> Self {
        let m = r * c;
        let mut planner = FftPlanner::<f32>::new();
        let plans = [
            planner.plan_fft_forward(r),
            planner.plan_fft_forward(c),
            planner.plan_fft_inverse(r),
            planner.plan_fft_inverse(c),
        ];
        let scratch = plans
            .iter()
            .map(|p| p.get_inplace_scratch_len())
            .max()
            .unwrap_or(0);
        let twiddle = (0..m)
            .map(|j| {
                let (sin, cos) = (-TAU * j as f64 / m as f64).sin_cos();
                Complex32::new(cos as f32, sin as f32)
            })
            .collect();
        Self {
            r,
            c,
            twiddle,
            plans,
            scratch: vec![Complex32::new(0.0, 0.0); scratch],
            line: vec![Complex32::new(0.0, 0.0); r.max(c)],
            calls: 0,
        }
    }

    /// The steps in a transform: `c` over the columns, then `r` over the rows.
    pub fn steps(&self) -> usize {
        self.r + self.c
    }

    /// How many steps have run, for the tests that check the work is flat.
    pub fn calls(&self) -> u64 {
        self.calls
    }

    /// Runs step `s` of a transform. `get(i)` is the input's point `i`; `t`
    /// is `len()` points of working space that the steps of one transform
    /// share, from first to last; `put(k, v)` receives the output's point
    /// `k`, which the row steps write. The inverse is unnormalised.
    pub fn step(
        &mut self,
        inverse: bool,
        s: usize,
        get: impl Fn(usize) -> Complex32,
        t: &mut [Complex32],
        mut put: impl FnMut(usize, Complex32),
    ) {
        let (r, c) = (self.r, self.c);
        let base = if inverse { 2 } else { 0 };
        self.calls += 1;
        if s < c {
            // Column `s`: points `s`, `s + c`, ... of the input.
            let line = &mut self.line[..r];
            for (n1, v) in line.iter_mut().enumerate() {
                *v = get(c * n1 + s);
            }
            self.plans[base].process_with_scratch(line, &mut self.scratch);
            for (k1, (out, &v)) in t[s * r..(s + 1) * r].iter_mut().zip(&*line).enumerate() {
                let w = self.twiddle[s * k1];
                *out = v * if inverse { w.conj() } else { w };
            }
        } else {
            // Row `k1`: one point from each column.
            let k1 = s - c;
            let line = &mut self.line[..c];
            for (n2, v) in line.iter_mut().enumerate() {
                *v = t[n2 * r + k1];
            }
            self.plans[base + 1].process_with_scratch(line, &mut self.scratch);
            for (k2, &v) in line.iter().enumerate() {
                put(k1 + r * k2, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every step, in order, gives the transform a whole-signal FFT gives, in
    /// both directions.
    #[test]
    fn steps_make_the_whole_transform() {
        for (r, c) in [(16, 8), (32, 32), (64, 64)] {
            let m = r * c;
            let x: Vec<Complex32> = (0..m)
                .map(|n| Complex32::new((n as f32 * 0.37).sin(), (n as f32 * 0.11).cos()))
                .collect();
            for inverse in [false, true] {
                let mut want = x.clone();
                let mut planner = FftPlanner::<f32>::new();
                let plan = if inverse {
                    planner.plan_fft_inverse(m)
                } else {
                    planner.plan_fft_forward(m)
                };
                plan.process(&mut want);
                let mut four = FourStep::new(r, c);
                let mut t = vec![Complex32::new(0.0, 0.0); m];
                let mut got = vec![Complex32::new(0.0, 0.0); m];
                for s in 0..four.steps() {
                    four.step(inverse, s, |i| x[i], &mut t, |k, v| got[k] = v);
                }
                let error = want
                    .iter()
                    .zip(&got)
                    .map(|(a, b)| (a - b).norm())
                    .fold(0.0, f32::max);
                assert!(error < 1e-3, "{r}x{c} inverse {inverse}: {error}");
                assert_eq!(four.calls(), (r + c) as u64);
            }
        }
    }
}
