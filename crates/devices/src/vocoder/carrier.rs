//! The Vocoder's carriers and the unvoiced detector (spec: vocoder R10 to
//! R12): the noise with its X–Y control, the band-limited oscillator the
//! pitch tracker drives, and the zero-crossing detector that tells a hiss
//! from a vowel.

use bl_dsp::Random;

/// The level carriers play at, so a full band of them leaves headroom.
pub const CARRIER_LEVEL: f64 = 0.5;

/// The lowest rate Noise's sample-and-hold is lowered to, in hertz (R10).
const LOWEST_RATE_HZ: f64 = 1_000.0;

/// How many samples, at most, pass between impulses at Noise's sparsest
/// density (R10).
const SPARSEST: f64 = 64.0;

/// Noise: white noise through a sample-and-hold whose rate X lowers and
/// whose density Y lowers (R10).
pub struct Noise {
    random: Random,
    phase: f64,
    held: f64,
}

impl Noise {
    /// A source following `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            random: Random::seeded(seed),
            phase: 1.0,
            held: 0.0,
        }
    }

    /// The next sample for the X–Y control at `x` and `y` (0 to 1 each):
    /// the hold's rate runs from the sample rate (`x` 0) down to 1 kHz
    /// (`x` 1), and an impulse falls at each tick of the hold with
    /// probability falling from 1 (`y` 0) to 1 in 64 (`y` 1).
    pub fn next(&mut self, x: f64, y: f64, rate: f64) -> f64 {
        let ticks = (LOWEST_RATE_HZ / rate).min(1.0).powf(x);
        self.phase += ticks;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            let density = 1.0 - y * (1.0 - 1.0 / SPARSEST);
            let draw = (self.random.next_bipolar() + 1.0) * 0.5;
            self.held = if draw < density {
                self.random.next_bipolar() * CARRIER_LEVEL
            } else {
                0.0
            };
        }
        self.held
    }

    /// One sample of plain white noise.
    pub fn white(&mut self) -> f64 {
        self.random.next_bipolar() * CARRIER_LEVEL
    }
}

/// The oscillator's waveform (R11): a sawtooth, or a pulse of duty 50, 25
/// or 12.5 %.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    Saw,
    Pulse(u8),
}

impl Wave {
    /// The waveform of the Waveform chooser's `index`.
    #[must_use]
    pub fn of(index: usize) -> Self {
        match index {
            0 => Self::Saw,
            n => Self::Pulse(n.min(3) as u8),
        }
    }

    /// A pulse's duty cycle.
    fn duty(n: u8) -> f64 {
        0.5 / f64::from(1u32 << (n - 1))
    }
}

/// How fast the oscillator's frequency follows its target: a time constant
/// in seconds, short enough to follow a voice and long enough to hide the
/// steps between the tracker's measurements.
const GLIDE_SECONDS: f64 = 0.003;

/// A band-limited oscillator (PolyBLEP) following a target frequency.
pub struct Oscillator {
    phase: f64,
    frequency: f64,
}

impl Oscillator {
    /// An oscillator at `frequency` hertz.
    #[must_use]
    pub fn new(frequency: f64) -> Self {
        Self {
            phase: 0.0,
            frequency,
        }
    }

    /// The next sample, the frequency moving toward `target`.
    pub fn next(&mut self, wave: Wave, target: f64, rate: f64) -> f64 {
        self.frequency += (target - self.frequency) * (1.0 - (-1.0 / (GLIDE_SECONDS * rate)).exp());
        let dt = (self.frequency / rate).clamp(1e-6, 0.45);
        let t = self.phase;
        let sample = match wave {
            Wave::Saw => 2.0 * t - 1.0 - blep(t, dt),
            Wave::Pulse(n) => {
                let duty = Wave::duty(n);
                let naive = if t < duty { 1.0 } else { -1.0 };
                naive + blep(t, dt) - blep((t + 1.0 - duty).fract(), dt) - (2.0 * duty - 1.0)
            }
        };
        self.phase = (self.phase + dt).fract();
        sample * CARRIER_LEVEL
    }
}

/// The residual that rounds a step at phase `t` for a phase increment `dt`.
fn blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = t / dt;
        2.0 * x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + 2.0 * x + 1.0
    } else {
        0.0
    }
}

/// The unvoiced detector (R12): per frame of 5 ms (Fast) or 30 ms (Slow), a
/// frame is unvoiced when its zero-crossing rate is above Sens.'s threshold
/// and its level above Gate.
pub struct Unvoiced {
    crossings: u32,
    energy: f64,
    count: usize,
    negative: bool,
    /// Whether the last frame was unvoiced.
    pub flag: bool,
    /// The noise's level, following `flag` over 1 ms.
    pub amount: f64,
}

/// The zero-crossing rates, in crossings per second over two, that Sens. at
/// just above 0 % (nearly never) and at 100 % put the threshold at.
const RATE_NEVER_HZ: f64 = 8_000.0;
const RATE_LOWEST_HZ: f64 = 500.0;

impl Unvoiced {
    /// A detector that has heard nothing.
    #[must_use]
    pub fn new() -> Self {
        Self {
            crossings: 0,
            energy: 0.0,
            count: 0,
            negative: false,
            flag: false,
            amount: 0.0,
        }
    }

    /// The zero-crossing rate, as a frequency, above which a frame is
    /// unvoiced at `sens` per cent; `None` at 0 %.
    #[must_use]
    pub fn threshold(sens: f64) -> Option<f64> {
        (sens > 0.0).then(|| RATE_NEVER_HZ * (RATE_LOWEST_HZ / RATE_NEVER_HZ).powf(sens * 0.01))
    }

    /// Hears one sample of the modulator. `frame` is the frame's length in
    /// samples, `sens` and `gate` (a linear amplitude) the controls.
    pub fn hear(&mut self, sample: f64, frame: usize, sens: f64, gate: f64, rate: f64) {
        let negative = sample < 0.0;
        if negative != self.negative {
            self.crossings += 1;
            self.negative = negative;
        }
        self.energy += sample * sample;
        self.count += 1;
        if self.count >= frame {
            let seconds = self.count as f64 / rate;
            let hertz = f64::from(self.crossings) * 0.5 / seconds;
            let level = (self.energy / self.count as f64).sqrt();
            self.flag = Self::threshold(sens).is_some_and(|t| hertz > t) && level > gate;
            self.crossings = 0;
            self.energy = 0.0;
            self.count = 0;
        }
        let target = if self.flag { 1.0 } else { 0.0 };
        self.amount += (target - self.amount) * (1.0 - (-1.0 / (0.001 * rate)).exp());
    }
}

impl Default for Unvoiced {
    fn default() -> Self {
        Self::new()
    }
}
