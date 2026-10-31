//! Physical models: modal resonators and waveguides, the sound of an object
//! that rings when something strikes, plucks, bows or blows it (the physical
//! models spec, R1 to R18).
//!
//! - [`ModalTable`] holds one shape's partials: [`Shape`]'s ratios, the
//!   [`Quality`]'s count, inharmonicity, and the mode shapes the excitation
//!   and listening positions read. It allocates, so it's built off the audio
//!   thread and handed to [`ModalBank::set_table`].
//! - [`ModalBank`] is a bank of modal resonators, one per voice, each a bank
//!   of decaying sine partials.
//! - [`WaveguideBank`] is a bank of delay-loop waveguides, one per voice:
//!   [`Kind::String`], [`Kind::Tube`] or [`Kind::Pipe`].
//!
//! Both banks run their voices in the plug-in kit's SIMD lanes, one voice per
//! lane, and are laid out as the kit's filter bank is: a bank's stride is its
//! voice count rounded up to a multiple of 16; the excitation is `frames ×
//! stride` values, one per voice per frame; the output is `frames × 2 ×
//! stride`, the left listening position's frame then the right's.
//!
//! Everything that runs per sample allocates nothing, locks nothing and
//! never blocks. Controls move on a grid of [`CONTROL`] frames counted from
//! the start of the bank's life, whatever the block size, so a render is the
//! same at every block size (R16).

mod modal;
mod shapes;
mod waveguide;

pub use modal::{ModalBank, ModalParams};
pub use shapes::{ModalTable, Quality, Shape};
pub use waveguide::{Interaction, Kind, WaveguideBank, WaveguideParams};

#[cfg(test)]
mod tests;

/// Frames between control updates: smoothing steps, coefficient refreshes
/// and the flush of tiny state.
pub const CONTROL: usize = 32;

/// Seconds a moving control takes to cover most of its way (R17).
const SMOOTH_SECONDS: f64 = 0.01;

/// Natural log of 1,000: 60 dB in amplitude.
const LN_1000: f64 = 6.907_755_278_982_137;

/// A voice state this small is flushed to zero (R14): 240 dB below full scale.
const TINY: f32 = 1e-12;

/// A voice state this large has blown: the excitation was not finite.
const BLOWN: f32 = 1e9;

/// A value clamped to `lo..=hi`, or `fallback` when it isn't a number.
fn sane(value: f32, lo: f32, hi: f32, fallback: f32) -> f64 {
    f64::from(if value.is_nan() {
        fallback
    } else {
        value.clamp(lo, hi)
    })
}

/// Moves a state value to zero if it's tiny or has blown.
#[inline]
fn flush(value: &mut f32) {
    let magnitude = value.abs();
    if !(TINY..=BLOWN).contains(&magnitude) {
        *value = 0.0;
    }
}

/// A voice's controls and where they are heading: a one-pole smoother on a
/// grid of [`CONTROL`] frames, settling exactly on its target.
#[derive(Clone, Copy, Debug)]
struct Controls<const N: usize> {
    target: [f64; N],
    current: [f64; N],
}

impl<const N: usize> Controls<N> {
    fn new(values: [f64; N]) -> Self {
        Self {
            target: values,
            current: values,
        }
    }

    /// One smoothing step of coefficient `alpha`; the bit set of the
    /// controls that moved (bit `i` for control `i`).
    fn step(&mut self, alpha: f64) -> u32 {
        let mut moved = 0;
        for i in 0..N {
            let gap = self.target[i] - self.current[i];
            if gap != 0.0 {
                moved |= 1 << i;
                self.current[i] = if gap.abs() < 1e-5 {
                    self.target[i]
                } else {
                    self.current[i] + gap * alpha
                };
            }
        }
        moved
    }
}

/// The smoothing coefficient for one control step at `rate`.
fn smoothing(rate: f64) -> f64 {
    1.0 - (-(CONTROL as f64) / (rate * SMOOTH_SECONDS)).exp()
}

/// The number of voices' lanes a bank pads to: the widest the kit's lanes go.
const LANES: usize = 16;
