//! Operator's LFO (WP-8.13; spec: operator R23): seven band-limited waves
//! at a rate in hertz, read a frame at a time so its output is the same at
//! any block size.

use bl_dsp::Random;

/// The waves, in the Wave chooser's order (R23).
pub const WAVES: [&str; 7] = [
    "Sine",
    "Square",
    "Triangle",
    "Saw Up",
    "Saw Down",
    "Sample & Hold",
    "Noise",
];
const SQUARE: usize = 1;
const TRIANGLE: usize = 2;
const SAW_UP: usize = 3;
const SAW_DOWN: usize = 4;
const SAMPLE_HOLD: usize = 5;
const NOISE: usize = 6;

/// The Range chooser's names: L for Low, H for Hi and S for Sync (R23), as
/// Live's chooser, which is 17 points wide, reads L.
pub const RANGES: [&str; 3] = ["L", "H", "S"];
pub const SYNC: usize = 2;

/// Spec: operator R23. Range Low runs from 50 seconds a cycle to 30 Hz, Hi
/// from 8 Hz to 12 kHz, each logarithmically over Rate's 0 to 100.
const LOW: (f64, f64) = (0.02, 30.0);
const HI: (f64, f64) = (8.0, 12_000.0);

/// Sync's divisions in beats a cycle, from 8 bars to 1/64 (Beatlayer's own
/// list): Rate's 0 to 100 picks one.
pub const DIVISIONS: [f64; 10] = [32.0, 16.0, 8.0, 4.0, 2.0, 1.0, 0.5, 0.25, 0.125, 0.0625];

/// The cycles a frame advances at `rate` (0 to 100) in `range` (an index of
/// [`RANGES`]), given the sample rate and the song's tempo.
#[must_use]
pub fn cycles(range: usize, rate: f64, sample_rate: f64, bpm: f64) -> f64 {
    let at = (rate / 100.0).clamp(0.0, 1.0);
    if range == SYNC {
        let division = DIVISIONS[(at * (DIVISIONS.len() - 1) as f64).round() as usize];
        return bpm / 60.0 / division / sample_rate;
    }
    let (low, high) = if range == 0 { LOW } else { HI };
    low * (high / low).powf(at) / sample_rate
}

/// The correction of an edge at phase 0 of height 2, `dt` being the
/// cycles a frame advances (PolyBLEP).
fn blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// One LFO: its phase, the value Sample & Hold holds, Noise's band-pass
/// state and its own random source, so what a voice's LFO plays never
/// depends on another voice or on how blocks fall.
#[derive(Clone)]
pub struct Lfo {
    phase: f64,
    held: f64,
    last: f64,
    band: [f64; 2],
    random: Random,
}

impl Lfo {
    /// An LFO drawing from `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            phase: 0.0,
            held: 0.0,
            last: 0.0,
            band: [0.0; 2],
            random: Random::seeded(seed),
        }
    }

    /// Starts the cycle again at `phase` (0 to 1), drawing from `seed`, with
    /// a value drawn for Sample & Hold.
    pub fn restart(&mut self, phase: f64, seed: u64) {
        self.phase = phase;
        self.random = Random::seeded(seed);
        self.last = self.random.next_bipolar();
        self.held = self.last;
        self.band = [0.0; 2];
    }

    /// The next frame, from -1 to 1, of `wave` advancing `dt` cycles.
    pub fn step(&mut self, wave: usize, dt: f64) -> f32 {
        let dt = dt.clamp(1e-9, 0.45);
        let t = self.phase;
        let value = match wave {
            SQUARE => {
                let naive = if t < 0.5 { 1.0 } else { -1.0 };
                naive + blep(t, dt) - blep((t + 0.5).fract(), dt)
            }
            TRIANGLE => 1.0 - 4.0 * (t - 0.5).abs(),
            SAW_UP => 2.0 * t - 1.0 - blep(t, dt),
            SAW_DOWN => 1.0 - 2.0 * t + blep(t, dt),
            // shortcut: only the half of the step after its edge is smoothed, as the
            // next value isn't drawn yet; upgrade by drawing it a cycle ahead.
            SAMPLE_HOLD if t < dt => self.held + (self.held - self.last) / 2.0 * blep(t, dt),
            SAMPLE_HOLD => self.held,
            NOISE => {
                // A state-variable band-pass around the rate, fed white
                // noise (Q of 1).
                let f = 2.0 * (core::f64::consts::PI * dt.min(1.0 / 6.0)).sin();
                let [low, band] = &mut self.band;
                let high = self.random.next_bipolar() - *low - *band;
                *band += f * high;
                *low += f * *band;
                (*band * 2.0).clamp(-1.0, 1.0)
            }
            _ => (core::f64::consts::TAU * t).sin(),
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
            self.last = self.held;
            self.held = self.random.next_bipolar();
        }
        value as f32
    }
}
