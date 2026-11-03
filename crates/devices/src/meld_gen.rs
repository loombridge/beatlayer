//! Meld's noise and texture types (spec: meld R30, R33, R35): Tarp, Filtered
//! Noise, Crackle, Rain and Bubble, each a generator a slot owns, drawing
//! its randomness from a generator seeded per device, engine, voice and
//! note, never from time, so a render repeats run after run and at any
//! block size.
//!
//! A generator renders one chunk of the control grid at a time from the
//! pitch at its ends and the two macros, which the voice holds between
//! grid points. These types are filtered rather than band-limited tables:
//! Tarp's comb has its low-pass, Filtered Noise its band-pass, and the
//! random drops and clicks are sines and decays that are kept below
//! Nyquist.

use std::f32::consts::{LN_2, PI, TAU};

use bl_dsp::random::Random;

use crate::meld_osc::CHUNK;

/// The types this plays, by their place in the type list (R4).
pub const TARP: usize = 16;
pub const FILTERED_NOISE: usize = 19;
pub const CRACKLE: usize = 21;
pub const RAIN: usize = 22;
pub const BUBBLE: usize = 23;

/// Whether type `index` is one of these.
#[must_use]
pub const fn plays(index: usize) -> bool {
    matches!(index, TARP | FILTERED_NOISE | CRACKLE | RAIN | BUBBLE)
}

/// The lowest pitch Tarp's comb tunes to, in hertz: the delay line holds
/// one period of it.
const COMB_LOW: f32 = 8.0;

/// The most drops of rain or bubbles sounding at once in a slot.
const DROPS: usize = 6;

/// A drop's or bubble's top frequency as a share of the sample rate.
const DROP_TOP: f32 = 0.4;

/// A sine that fades and rises in pitch: a drop of rain or a bubble.
#[derive(Clone, Copy, Default)]
struct Drop {
    x: f32,
    y: f32,
    /// Its rotation per sample, and its angle in radians per sample.
    cos: f32,
    sin: f32,
    angle: f32,
    level: f32,
    /// What its level is multiplied by each frame, and its angle each
    /// chunk.
    fall: f32,
    rise: f32,
}

/// A state-variable band-pass's two integrators and its coefficients.
#[derive(Clone, Copy, Default)]
struct Band {
    ic1: f32,
    ic2: f32,
}

/// The coefficients of a band-pass at `centre` hertz and quality `q`:
/// (a1, a2, a3, k).
fn band_coefficients(centre: f32, q: f32, rate: f32) -> [f32; 4] {
    let g = (PI * centre / rate).tan();
    let k = 1.0 / q;
    let a1 = 1.0 / (1.0 + g * (g + k));
    [a1, g * a1, g * g * a1, k]
}

impl Band {
    /// `x` through the band-pass, at a gain of one at its centre.
    fn tick(&mut self, x: f32, [a1, a2, a3, k]: &[f32; 4]) -> f32 {
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        k * v1
    }
}

/// The note's pitch over the frames being rendered from a chunk of the
/// control grid: what the chunk's coefficients follow, so that they are the
/// same wherever a block cuts the chunk, and each frame's frequency.
#[derive(Clone, Copy)]
pub struct Pitch<'a> {
    /// The frequency at the chunk's start.
    pub chunk: f32,
    /// The pitch in semitones at the first frame rendered, and its change a
    /// frame (a glide's line).
    pub first: f32,
    pub step: f32,
    /// Every frame's frequency, where a Glissando's steps move it within a
    /// chunk.
    pub exact: Option<&'a [f32]>,
    /// Where the frames start in the chunk.
    pub offset: usize,
}

impl Pitch<'_> {
    /// The frequency at frame `k` of the frames being rendered.
    #[must_use]
    pub fn at(&self, k: usize) -> f32 {
        self.exact.map_or_else(
            || 440.0 * ((self.first + self.step * k as f32 - 69.0) / 12.0).exp2(),
            |e| e[k],
        )
    }

    /// Whether the frames start the chunk.
    fn starts(&self) -> bool {
        self.offset == 0
    }
}

/// One slot's generator.
pub struct Gen {
    random: Random,
    rate: f32,
    /// Tarp: the comb's delay line, where it writes next, its loop's
    /// low-pass, and the burst still to excite it with (`None` before the
    /// note's first frame).
    comb: Vec<f32>,
    head: usize,
    damp: f32,
    burst: Option<usize>,
    /// Filtered Noise's band, Rain's wind: a low-pass and a band-pass.
    band: Band,
    wind: f32,
    /// Crackle: the click's decay, and what it passes through.
    click: f32,
    shade: f32,
    smooth: f32,
    drops: [Drop; DROPS],
    next: usize,
}

impl Gen {
    /// A generator for `rate`. Allocates its comb.
    #[must_use]
    pub fn new(rate: f32) -> Self {
        Self {
            random: Random::seeded(1),
            rate,
            comb: vec![0.0; (rate / COMB_LOW) as usize + 4],
            head: 0,
            damp: 0.0,
            burst: None,
            band: Band::default(),
            wind: 0.0,
            click: 0.0,
            shade: 0.5,
            smooth: 0.0,
            drops: [Drop::default(); DROPS],
            next: 0,
        }
    }

    /// Starts a note: forgets what sounded and draws from `seed`. Never
    /// allocates.
    pub fn start(&mut self, seed: u64) {
        self.random = Random::seeded(seed);
        self.comb.fill(0.0);
        self.head = 0;
        self.damp = 0.0;
        self.burst = None;
        self.band = Band::default();
        self.wind = 0.0;
        self.click = 0.0;
        self.smooth = 0.0;
        self.drops = [Drop::default(); DROPS];
    }

    fn unit(&mut self) -> f32 {
        (self.random.next_bipolar() as f32 + 1.0) * 0.5
    }

    fn bipolar(&mut self) -> f32 {
        self.random.next_bipolar() as f32
    }

    /// A draw with a spread of one: three uniform draws added.
    fn normal(&mut self) -> f32 {
        self.bipolar() + self.bipolar() + self.bipolar()
    }

    /// Renders type `index` into `out`, frames of a chunk of the control grid
    /// (`pitch.offset + out.len()` is at most [`CHUNK`]) with the note at
    /// `pitch` and macros `m` (0 to 1).
    pub fn render(&mut self, index: usize, out: &mut [f32], pitch: &Pitch<'_>, m: (f32, f32)) {
        debug_assert!(pitch.offset + out.len() <= CHUNK);
        match index {
            TARP => self.tarp(out, pitch, m),
            FILTERED_NOISE => self.filtered(out, pitch, m),
            CRACKLE => self.crackle(out, m),
            RAIN => self.rain(out, pitch, m),
            _ => self.bubble(out, pitch, m),
        }
    }

    /// Spec: meld R30. A burst of noise one period long from the seed into
    /// a comb tuned to the note: Decay (10 ms to 4 s) sets how long it
    /// rings and Tone (the loop's low-pass, from 1.5 to 48 times the note)
    /// how bright, the low-pass's delay taken out of the comb's.
    fn tarp(&mut self, out: &mut [f32], pitch: &Pitch<'_>, (decay, tone): (f32, f32)) {
        let rate = self.rate;
        let len = self.comb.len();
        let mid = pitch.chunk;
        let cutoff = (mid * 1.5 * 32f32.powf(tone)).min(0.45 * rate);
        let smooth = 1.0 - (-TAU * cutoff / rate).exp();
        let lag = (1.0 - smooth) / smooth;
        let ring = 0.01 * 400f32.powf(decay);
        for (k, sample) in out.iter_mut().enumerate() {
            let delay = (rate / pitch.at(k) - lag).clamp(2.0, (len - 2) as f32);
            let feedback = 10f32.powf(-3.0 * delay / (rate * ring));
            let burst = *self.burst.get_or_insert(delay as usize + 1);
            let excite = if burst > 0 {
                self.burst = Some(burst - 1);
                self.bipolar()
            } else {
                0.0
            };
            let from = (self.head + len) as f32 - delay;
            let (i, f) = (from as usize, from.fract());
            let (a, b) = (self.comb[i % len], self.comb[(i + 1) % len]);
            self.damp += smooth * (a + (b - a) * f - self.damp);
            let y = excite + feedback * self.damp;
            self.comb[self.head] = y;
            self.head = (self.head + 1) % len;
            *sample = y;
        }
    }

    /// Spec: meld R33. White noise through a band-pass at the note times
    /// Freq (0.25 to 4) a width of Narrow's four octaves down to a
    /// twentieth of one, at a level that doesn't move with the width.
    fn filtered(&mut self, out: &mut [f32], pitch: &Pitch<'_>, (freq, narrow): (f32, f32)) {
        let rate = self.rate;
        let centre = (pitch.chunk * 0.25 * 16f32.powf(freq)).clamp(20.0, 0.45 * rate);
        let octaves = 4.0 * 0.0125f32.powf(narrow);
        let q = 1.0 / (2.0 * (LN_2 * 0.5 * octaves).sinh());
        let coefficients = band_coefficients(centre, q, rate);
        let level = (q * rate / (PI * centre)).sqrt().min(30.0) * 0.5;
        for sample in out {
            let white = self.bipolar();
            *sample = self.band.tick(white, &coefficients) * level;
        }
    }

    /// Spec: meld R35. Clicks at random times, Density (1 to 2,000) a
    /// second on average, each a decaying impulse (1.5 ms) of a random sign
    /// whose loudness and brightness are drawn from a distribution that
    /// Intensity widens.
    fn crackle(&mut self, out: &mut [f32], (density, intensity): (f32, f32)) {
        let rate = self.rate;
        let chance = 2000f32.powf(density) / rate;
        let decay = (-1.0 / (0.0015 * rate)).exp();
        for sample in out {
            if self.unit() < chance {
                let loud = (2.5 * intensity * self.normal()).exp().clamp(0.05, 3.0);
                let sign = self.bipolar().signum();
                self.click += sign * loud;
                self.shade = (0.5 * (1.5 * intensity * self.normal()).exp()).clamp(0.02, 1.0);
            }
            self.click *= decay;
            self.smooth += self.shade * (self.click - self.smooth);
            *sample = self.smooth * 0.2;
        }
    }

    /// Starts a drop at `hz` hertz, level `level`, for a fall of `seconds`
    /// and a rise in pitch of `rise` over it, in the oldest place.
    fn drop(&mut self, hz: f32, level: f32, seconds: f32, rise: f32) {
        let rate = self.rate;
        let angle = TAU * hz.min(DROP_TOP * rate) / rate;
        self.drops[self.next] = Drop {
            x: 1.0,
            y: 0.0,
            cos: angle.cos(),
            sin: angle.sin(),
            angle,
            level,
            fall: (-1.0 / (seconds * rate)).exp(),
            rise: (rise.ln() * CHUNK as f32 / (seconds * rate)).exp(),
        };
        self.next = (self.next + 1) % DROPS;
    }

    /// Moves every drop's pitch on a step of the grid.
    fn glide_drops(&mut self) {
        for drop in &mut self.drops {
            if drop.level > 1e-4 {
                drop.angle = (drop.angle * drop.rise).min(TAU * DROP_TOP);
                drop.cos = drop.angle.cos();
                drop.sin = drop.angle.sin();
                let size = drop.x.hypot(drop.y).recip();
                drop.x *= size;
                drop.y *= size;
            }
        }
    }

    /// The sum of the sounding drops over one frame.
    fn sound_drops(&mut self) -> f32 {
        let mut sum = 0.0;
        for drop in &mut self.drops {
            if drop.level > 1e-4 {
                let x = drop.x * drop.cos - drop.y * drop.sin;
                drop.y = drop.x * drop.sin + drop.y * drop.cos;
                drop.x = x;
                sum += drop.y * drop.level;
                drop.level *= drop.fall;
            }
        }
        sum
    }

    /// Spec: meld R35. Drops (short sines whose pitch rises, 12 ms), Rate
    /// (1 to 500) a second, over wind: noise through a low-pass and a
    /// band-pass at the note, Tone from the low-pass and drops at random
    /// pitches (zero) to the band-pass and drops at the note (one).
    fn rain(&mut self, out: &mut [f32], pitch: &Pitch<'_>, (tone, rate_macro): (f32, f32)) {
        let rate = self.rate;
        let note = pitch.chunk;
        let chance = 500f32.powf(rate_macro) / rate;
        let coefficients = band_coefficients(note.clamp(20.0, 0.45 * rate), 6.0, rate);
        let low = 1.0 - (-TAU * 1_000.0 / rate).exp();
        let wind = (6.0 * rate / (PI * note.clamp(20.0, 0.45 * rate)))
            .sqrt()
            .min(30.0);
        if pitch.starts() {
            self.glide_drops();
        }
        for sample in out {
            if self.unit() < chance {
                let hz = note * ((1.0 - tone) * 1.5 * self.bipolar()).exp2();
                let level = 0.3 + 0.7 * self.unit();
                self.drop(hz, level, 0.012, 1.5);
            }
            let white = self.bipolar();
            self.wind += low * (white - self.wind);
            let band = self.band.tick(white, &coefficients);
            let wind = (1.0 - tone) * self.wind * 2.0 + tone * band * wind * 0.2;
            *sample = (self.sound_drops() + 0.25 * wind) * 0.5;
        }
    }

    /// Spec: meld R35. Bubbles (sines chirping upward as they decay, 20 ms),
    /// Density (1 to 200) a second, each one's size, and so its pitch,
    /// varied by up to an octave around the note by Spread.
    fn bubble(&mut self, out: &mut [f32], pitch: &Pitch<'_>, (density, spread): (f32, f32)) {
        let chance = 200f32.powf(density) / self.rate;
        let note = pitch.chunk;
        if pitch.starts() {
            self.glide_drops();
        }
        for sample in out {
            if self.unit() < chance {
                let hz = note * (spread * self.bipolar()).exp2();
                let level = 0.3 + 0.7 * self.unit();
                self.drop(hz, level, 0.02, 1.6);
            }
            *sample = self.sound_drops() * 0.5;
        }
    }
}
