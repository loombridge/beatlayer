//! The parts of Roar's second half (WP-8.39; the roar spec R21 to R29): the
//! two LFOs, the envelope follower and the noise source that feed the
//! matrix, the 32-frame grid they are worked out on, the feedback path's
//! band-pass and gate, and the compressor.
//!
//! Everything here is a plain value made with the device, so nothing in it
//! allocates on the audio thread.

use core::f64::consts::{LN_2, PI, TAU};

use bl_core::time::{SampleRate, TempoMap};
use bl_dsp::{EnvelopeFollower, Random};

use crate::dynamics::{curve_db, level_db};
use crate::framework::Audio;
use crate::lfo::{Lfo, Rate, Shape, Stereo, Wave};
use bl_model::param::db_to_gain;

/// Frames between the grid points the sources are worked out at (the
/// modulation spec, R20).
pub(crate) const GRID: u64 = 32;

/// A source's value on the grid: from the point before to the point now,
/// reached over the next 32 frames in a straight line, so a target never
/// steps and a render doesn't depend on where the blocks fall.
#[derive(Clone, Copy, Default)]
pub(crate) struct Glide {
    from: f64,
    to: f64,
}

impl Glide {
    /// Takes the value worked out at a grid point.
    pub(crate) fn push(&mut self, value: f64) {
        self.from = self.to;
        self.to = value;
    }

    /// The value `phase` frames (0 to 31) after the grid point.
    pub(crate) fn at(&self, phase: u64) -> f64 {
        self.from + (self.to - self.from) * phase as f64 / GRID as f64
    }
}

/// An LFO's waves, in the chooser's order.
const WAVES: [Wave; 5] = [
    Wave::Sine,
    Wave::Triangle,
    Wave::Rectangle,
    Wave::SawUp,
    Wave::SawDown,
];

/// One of the two LFOs: the effects LFO's left voice, a slew, and the grid.
pub(crate) struct LfoSource {
    lfo: Lfo,
    rate: f64,
    smooth: f64,
}

/// What an LFO is set to at a grid point.
#[derive(Clone, Copy)]
pub(crate) struct LfoSettings {
    pub(crate) wave: usize,
    pub(crate) rate: Rate,
    pub(crate) morph: f64,
    pub(crate) smooth: f64,
}

impl LfoSource {
    pub(crate) fn new(rate: SampleRate, seed: u64) -> Self {
        Self {
            lfo: Lfo::new(rate, seed),
            rate: rate.as_f64(),
            smooth: 0.0,
        }
    }

    /// Sets its rate for the stretch in `audio`.
    pub(crate) fn begin(&mut self, audio: &Audio<'_, '_>, rate: Rate) {
        self.lfo.set(audio, rate, Stereo::Phase(0.0), 0.0);
    }

    /// Its value at the grid point `frame` frames into the stretch.
    pub(crate) fn tick(
        &mut self,
        tempo: &TempoMap,
        frame: usize,
        playing: bool,
        bpm: f64,
        s: LfoSettings,
    ) -> f64 {
        let shape = Shape {
            wave: WAVES[s.wave.min(WAVES.len() - 1)],
            morph: None,
            duty: s.morph,
        };
        // A rate in quarter notes locks to the song while it plays; every
        // other rate moves on by the grid's 32 frames.
        let locked = playing && matches!(s.rate, Rate::Beats(_));
        let multiplier = if locked { 1.0 } else { GRID as f64 };
        let value = self
            .lfo
            .step_with_multiplier(tempo, frame, shape, multiplier)[0];
        let hz = match s.rate {
            Rate::Hertz(hz) => hz,
            Rate::Seconds(seconds) => 1.0 / seconds.max(1e-9),
            Rate::Beats(beats) => bpm / 60.0 / beats.max(1e-9),
        };
        // Smooth: a slew of up to a quarter of a cycle.
        let cycles = s.smooth * 0.25;
        let step = if cycles <= 0.0 || hz <= 0.0 {
            1.0
        } else {
            1.0 - (-(GRID as f64) * hz / (self.rate * cycles)).exp()
        };
        self.smooth += (value - self.smooth) * step;
        self.smooth
    }
}

/// A hash of a lattice point and a seed, from −1 to 1.
fn gradient(seed: u64, at: i64) -> f64 {
    let mut z = seed ^ (at as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// The noise source: Simplex, Wander, S & H or Brown, from the device's seed.
pub(crate) struct Noise {
    random: Random,
    seed: u64,
    phase: f64,
    held: f64,
    next: f64,
    brown: f64,
    rate: f64,
}

impl Noise {
    pub(crate) fn new(rate: SampleRate, seed: u64) -> Self {
        let mut random = Random::seeded(seed);
        let (held, next) = (random.next_bipolar(), random.next_bipolar());
        Self {
            random,
            seed,
            phase: 0.0,
            held,
            next,
            brown: 0.0,
            rate: rate.as_f64(),
        }
    }

    /// Its value at the next grid point, `kind` one of the chooser's four,
    /// at `hz` cycles per second.
    pub(crate) fn tick(&mut self, kind: usize, hz: f64) -> f64 {
        let advance = hz * GRID as f64 / self.rate;
        self.phase += advance;
        if kind == 0 {
            let whole = self.phase.floor();
            let f = self.phase - whole;
            let at = whole as i64;
            let fade = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
            let (a, b) = (
                gradient(self.seed, at) * f,
                gradient(self.seed, at + 1) * (f - 1.0),
            );
            // 1-D gradient noise peaks at one half.
            return (2.0 * (a + (b - a) * fade)).clamp(-1.0, 1.0);
        }
        if kind == 3 {
            self.brown = (self.brown + self.random.next_bipolar() * advance.min(1.0).sqrt())
                .clamp(-1.0, 1.0);
            return self.brown;
        }
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.held = self.next;
            self.next = self.random.next_bipolar();
        }
        if kind == 1 {
            self.held + (self.next - self.held) * self.phase
        } else {
            self.held
        }
    }
}

/// A second-order section by the RBJ cookbook, in double precision: the
/// feedback path's band-pass and the compressor's sidechain high-pass.
#[derive(Clone, Copy, Default)]
pub(crate) struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    fn set(&mut self, b: [f64; 3], a: [f64; 3]) {
        self.b0 = b[0] / a[0];
        self.b1 = b[1] / a[0];
        self.b2 = b[2] / a[0];
        self.a1 = a[1] / a[0];
        self.a2 = a[2] / a[0];
    }

    /// A band-pass with a 0 dB peak at `hz`, `octaves` wide; gives its
    /// centre in radians per sample and its Q.
    pub(crate) fn band_pass(&mut self, hz: f64, octaves: f64, rate: f64) -> (f64, f64) {
        let w = TAU * hz.clamp(1.0, rate * 0.45) / rate;
        let (sin, cos) = w.sin_cos();
        let q = 1.0 / (2.0 * (0.5 * LN_2 * octaves * w / sin).sinh());
        let alpha = sin / (2.0 * q);
        self.set([alpha, 0.0, -alpha], [1.0 + alpha, -2.0 * cos, 1.0 - alpha]);
        (w, q)
    }

    /// A 12 dB per octave high-pass at `hz`.
    pub(crate) fn high_pass(&mut self, hz: f64, rate: f64) {
        let w = TAU * hz.min(rate * 0.45) / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * core::f64::consts::FRAC_1_SQRT_2);
        let k = 0.5 * (1.0 + cos);
        self.set([k, -2.0 * k, k], [1.0 + alpha, -2.0 * cos, 1.0 - alpha]);
    }

    pub(crate) fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = if y.abs() < 1e-30 { 0.0 } else { y };
        y
    }

    /// The delay in samples its phase gives a tone at `w` radians per
    /// sample.
    pub(crate) fn phase_delay(&self, w: f64) -> f64 {
        let (c1, s1) = (w.cos(), -w.sin());
        let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num = (
            self.b0 + self.b1 * c1 + self.b2 * c2,
            self.b1 * s1 + self.b2 * s2,
        );
        let den = (
            1.0 + self.a1 * c1 + self.a2 * c2,
            self.a1 * s1 + self.a2 * s2,
        );
        let phase = num.1.atan2(num.0) - den.1.atan2(den.0);
        let phase = (phase + PI).rem_euclid(TAU) - PI;
        -phase / w.max(1e-9)
    }
}

/// The envelope follower source: a band-pass at Frequency and Width, Gain,
/// then a follower with Attack, Release and Hold; it gives 0 below Threshold
/// and rises with the level above it.
pub(crate) struct EnvSource {
    filters: [Biquad; 2],
    follower: EnvelopeFollower,
    gain: f64,
    threshold: f64,
}

impl EnvSource {
    pub(crate) fn new(rate: f64) -> Self {
        Self {
            filters: [Biquad::default(); 2],
            follower: EnvelopeFollower::new(rate),
            gain: 1.0,
            threshold: -60.0,
        }
    }

    /// Sets it for a stretch: times in seconds, Gain as a factor, Threshold
    /// in dBFS, the band-pass at `hz` and `octaves` wide.
    pub(crate) fn set(
        &mut self,
        (attack, release, hold): (f64, f64, bool),
        (gain, threshold): (f64, f64),
        (hz, octaves): (f64, f64),
        rate: f64,
    ) {
        self.follower.set(attack, release, hold);
        self.gain = gain;
        self.threshold = threshold;
        for filter in &mut self.filters {
            filter.band_pass(hz, octaves, rate);
        }
    }

    /// Takes a frame, `left` and `right`, and gives what the detector heard
    /// there (for Input Listen).
    pub(crate) fn feed(&mut self, left: f32, right: f32) -> [f32; 2] {
        let heard = [
            self.filters[0].process(f64::from(left)) * self.gain,
            self.filters[1].process(f64::from(right)) * self.gain,
        ];
        self.follower
            .process(heard[0].abs().max(heard[1].abs()) as f32);
        [heard[0] as f32, heard[1] as f32]
    }

    /// The source's value now, from 0 to 1: 0 at Threshold, rising in dB to
    /// 1 over the span from there to 0 dBFS (at least 6 dB).
    pub(crate) fn value(&self) -> f64 {
        let span = (-self.threshold).max(6.0);
        ((level_db(self.follower.level()) - self.threshold) / span).clamp(0.0, 1.0)
    }
}

/// The feedback gate (R24): the feedback fades over 200 ms once the input
/// stays below −60 dBFS for 50 ms, and returns in 5 ms.
#[derive(Clone, Copy)]
pub(crate) struct Gate {
    quiet: u64,
    gain: f64,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            quiet: 0,
            gain: 1.0,
        }
    }
}

impl Gate {
    pub(crate) fn process(&mut self, input: f64, on: bool, rate: f64) -> f64 {
        if !on {
            self.quiet = 0;
            self.gain = 1.0;
            return 1.0;
        }
        if input.abs() > 0.001 {
            self.quiet = 0;
        } else {
            self.quiet += 1;
        }
        if self.quiet as f64 >= 0.05 * rate {
            self.gain = (self.gain - 1.0 / (0.2 * rate)).max(0.0);
        } else {
            self.gain = (self.gain + 1.0 / (0.005 * rate)).min(1.0);
        }
        self.gain
    }
}

/// The compressor's law at an amount (R27): the threshold falls from 0 to
/// −30 dBFS and the ratio rises from 1 : 1 to 8 : 1.
#[derive(Clone, Copy)]
pub(crate) struct Law {
    threshold: f64,
    slope: f64,
}

impl Law {
    pub(crate) fn of(amount: f64) -> Self {
        Self {
            threshold: -30.0 * amount,
            slope: 1.0 - 1.0 / (1.0 + 7.0 * amount),
        }
    }

    /// The gain in dB for a detector level.
    #[must_use]
    pub(crate) fn gain_db(self, level: f64) -> f64 {
        curve_db(level_db(level), self.threshold, self.slope, 0.0, false)
    }
}

/// The compressor (R27): a feed-forward peak detector, through a 150 Hz
/// high-pass when Sidechain HP is on, with a 10 ms attack and a 150 ms
/// release.
#[derive(Clone, Copy)]
pub(crate) struct Squash {
    hp: Biquad,
    gain: f64,
    attack: f64,
    release: f64,
    /// The lowest gain since it was last read, as a factor.
    least: f64,
}

/// The compressor's attack and release, in seconds.
const ATTACK: f64 = 0.010;
const RELEASE: f64 = 0.150;
/// The sidechain high-pass's corner.
const HP_HZ: f64 = 150.0;

impl Squash {
    pub(crate) fn new(rate: f64) -> Self {
        let mut hp = Biquad::default();
        hp.high_pass(HP_HZ, rate);
        Self {
            hp,
            gain: 1.0,
            attack: 1.0 - (-1.0 / (ATTACK * rate)).exp(),
            release: 1.0 - (-1.0 / (RELEASE * rate)).exp(),
            least: 1.0,
        }
    }

    /// Takes one output sample `y` and gives the gain to apply to it.
    pub(crate) fn process(&mut self, y: f64, law: Law, high_pass: bool) -> f64 {
        let heard = if high_pass { self.hp.process(y) } else { y };
        let target = db_to_gain(law.gain_db(heard.abs()));
        let step = if target < self.gain {
            self.attack
        } else {
            self.release
        };
        self.gain += (target - self.gain) * step;
        self.least = self.least.min(self.gain);
        self.gain
    }

    /// The gain taken off since the last call, in dB (zero or negative).
    pub(crate) fn take_reduction(&mut self) -> f64 {
        let db = level_db(core::mem::replace(&mut self.least, self.gain));
        db.min(0.0)
    }
}
