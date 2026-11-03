//! The parts DS HH, DS Cymbal and DS Clang share (WP-8.25; the drum synths
//! spec, R15 to R19): square waves that stay band-limited at every pitch,
//! and white and pink noise.

use core::f32::consts::TAU;

use bl_dsp::random::Random;
use lspk_wavetable::{Cache, FRAME_SIZE, SharedTable, Wavetable, increment};

/// The odd harmonics a square's table holds: every one below its frame's
/// Nyquist. A note reads the mipmap level that keeps only those below the
/// device's Nyquist, so a square never folds back at any pitch (spec R19).
const HARMONICS: usize = FRAME_SIZE / 2 - 1;

/// `N` square waves, each at its own frequency, read from one band-limited
/// table. Nothing here allocates after [`Squares::new`].
pub(crate) struct Squares<const N: usize> {
    table: SharedTable,
    phase: [u32; N],
}

impl<const N: usize> Squares<N> {
    /// Builds the table: call it off the audio thread.
    pub(crate) fn new() -> Self {
        let sine: Vec<f32> = (0..FRAME_SIZE)
            .map(|n| (f64::from(TAU) * n as f64 / FRAME_SIZE as f64).sin() as f32)
            .collect();
        let square = Wavetable::from_fn(1, |_, n| {
            (1..=HARMONICS)
                .step_by(2)
                .map(|h| 4.0 / core::f32::consts::PI / h as f32 * sine[(h * n) % FRAME_SIZE])
                .sum()
        })
        .expect("one frame is a table");
        Self {
            table: Cache::global().share(square),
            phase: [0; N],
        }
    }

    /// Starts every square at the beginning of its cycle.
    pub(crate) fn start(&mut self) {
        self.phase = [0; N];
    }

    /// One frame of each square at `hz` hertz, at `rate`, from -1 to 1.
    pub(crate) fn tick(&mut self, hz: [f64; N], rate: f64) -> [f32; N] {
        let mut out = [0.0; N];
        for ((out, phase), hz) in out.iter_mut().zip(&mut self.phase).zip(hz) {
            let step = increment(hz, rate);
            *out = self.table.wave(self.table.level_for(step), 0).read(*phase);
            *phase = phase.wrapping_add(step);
        }
        out
    }
}

/// White noise or pink, from the device's seed, started again with each
/// hit (spec R8).
pub(crate) struct Noise {
    random: Random,
    /// The pink filter's three poles.
    poles: [f64; 3],
}

impl Noise {
    pub(crate) fn new() -> Self {
        Self {
            random: Random::seeded(0),
            poles: [0.0; 3],
        }
    }

    pub(crate) fn start(&mut self, seed: u64) {
        self.random = Random::seeded(seed);
        self.poles = [0.0; 3];
    }

    /// The next white sample, from -1 to 1.
    pub(crate) fn white(&mut self) -> f64 {
        self.random.next_bipolar()
    }

    /// The next pink sample: white noise through three poles whose sum falls
    /// about 3 dB per octave, scaled to a level near white's.
    pub(crate) fn pink(&mut self) -> f64 {
        const POLES: [(f64, f64); 3] = [
            (0.99765, 0.099_046),
            (0.963, 0.296_516_4),
            (0.57, 1.052_691_3),
        ];
        let white = self.white();
        let mut sum = white * 0.1848;
        for (pole, (keep, add)) in self.poles.iter_mut().zip(POLES) {
            *pole = keep * *pole + white * add;
            sum += *pole;
        }
        sum * 0.25
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: drum-synths R19
    #[test]
    fn squares_at_the_highest_pitch_keep_only_harmonics_below_nyquist_and_play_them_cleanly() {
        let rate = 48_000.0;
        // DS HH's six squares at Pitch +24: a base of 1,600 Hz.
        let hz = crate::ds_hh::RATIOS.map(|ratio| 1_600.0 * ratio);
        let mut squares = Squares::<6>::new();
        // The harmonics of the level each square reads: the mipmap steps
        // in thirds, so it can hold fewer than every one below Nyquist, and
        // never one at or above it.
        let kept = hz.map(|hz| {
            let level = squares.table.level_for(increment(hz, rate));
            squares.table.levels()[level].harmonics
        });
        for (kept, hz) in kept.into_iter().zip(hz) {
            assert!(
                kept as f64 * hz < rate / 2.0,
                "{hz} Hz keeps {kept} harmonics"
            );
            assert!(kept as f64 * hz > rate / 2.0 / 1.6, "{hz} Hz keeps {kept}");
        }
        let (mut error, mut ideal_power) = (0.0f64, 0.0f64);
        for n in 0..8_192 {
            let got = squares.tick(hz, rate);
            for ((got, hz), kept) in got.into_iter().zip(hz).zip(kept) {
                let ideal: f64 = (1..=kept)
                    .step_by(2)
                    .map(|h| {
                        let h = h as f64;
                        4.0 / std::f64::consts::PI / h
                            * (std::f64::consts::TAU * h * hz * n as f64 / rate).sin()
                    })
                    .sum();
                error += (f64::from(got) - ideal).powi(2);
                ideal_power += ideal.powi(2);
            }
        }
        let db = 10.0 * (error / ideal_power).log10();
        assert!(db < -80.0, "the error is {db:.1} dB below the square");
    }
}
