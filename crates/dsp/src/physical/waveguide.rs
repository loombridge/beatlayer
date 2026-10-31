//! A bank of digital waveguides: per voice, a delay loop that a wave travels
//! round, losing a little at each pass (physical models R8 to R11, R12 to
//! R17).
//!
//! The loop is, in order, an integer delay (a ring buffer), a first-order
//! Thiran all-pass for the fractional part of the delay, two first-order
//! all-passes that disperse (stretch or squeeze) the partials, and a
//! one-pole low-pass with a gain for the losses; the sign of the loop is the
//! sign of the two ends' reflections together. The design works the delay
//! out from the *phase* delay of each part at the fundamental, so the loop's
//! own filters don't detune the note, and sets the dispersion so a chosen
//! higher harmonic sits where the stiff-string law puts it (Smith, 1992;
//! Laakso et al., 1996).
//!
//! An excitation enters through a comb at its position along the guide and
//! each listening position reads through a comb too, so the harmonics are
//! shaped as a string struck and heard there is (R11): `1 − z^(−pM)` has a
//! zero at every harmonic whose sine has a node at `p`.

use lspk_dsp::simd::{self, F32s, Kernel, Level};

use super::{BLOWN, CONTROL, Controls, LANES, LN_1000, TINY, flush, sane, smoothing};
use std::f64::consts::PI;

/// What the guide is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Reflects with inverted sign at both ends: every harmonic.
    String,
    /// Closed at both ends, reflecting without inversion: every harmonic.
    Tube,
    /// Closed at one end, and at the other an end whose opening moves it
    /// from closed (odd harmonics, an octave below a tube of its length) to
    /// open (every harmonic).
    Pipe,
}

/// What a waveguide voice is set to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveguideParams {
    /// The note, in hertz.
    pub hz: f32,
    /// Seconds for the fundamental to fall 60 dB, 0.001 to 60 (R8).
    pub decay: f32,
    /// How much faster high harmonics die, from 0 (fast: wood, nylon) to 1
    /// (all alike) (R8). A string's Damping.
    pub material: f32,
    /// The stiffness, 0 to 1: the law's B is 0.001 at 1 (R8).
    pub inharm: f32,
    /// A pipe's opening, 0 to 1 (R9).
    pub opening: f32,
    /// A pipe's or tube's radius, 0 to 1 (R10).
    pub radius: f32,
    /// Where it's excited, from 0 (an end) to 0.5 (the middle) (R11).
    pub excite: f32,
    /// Where the left channel listens, 0 to 0.5.
    pub pickup_l: f32,
    /// Where the right channel listens, 0 to 0.5.
    pub pickup_r: f32,
}

impl Default for WaveguideParams {
    fn default() -> Self {
        Self {
            hz: 220.0,
            decay: 1.0,
            material: 0.5,
            inharm: 0.0,
            opening: 1.0,
            radius: 0.0,
            excite: 0.25,
            pickup_l: 0.25,
            pickup_r: 0.25,
        }
    }
}

/// A nonlinear interaction at the excitation position: a bow's friction, a
/// hammer's or a plectrum's contact (R11). It reads the guide's velocity
/// there and returns a force, which is added to the voice's excitation.
pub trait Interaction {
    /// Whether the bank needs to call it at all: `()` says no.
    const ACTIVE: bool = true;

    /// The force on `voice` when the guide moves at `velocity` there.
    fn force(&mut self, voice: usize, velocity: f32) -> f32;
}

impl Interaction for () {
    const ACTIVE: bool = false;

    fn force(&mut self, _voice: usize, _velocity: f32) -> f32 {
        0.0
    }
}

/// Every parameter moved by smoothing, in the order the controls hold them.
const CONTROLS: usize = 9;
const HZ: usize = 0;
const DECAY: usize = 1;
/// The bits of the controls that change the loop.
const LOOP_BITS: u32 = 0b0011_1111;

/// The loop's largest gain: below 1 at every frequency (R13).
const LOOP_GAIN: f64 = 0.9999;
/// The largest pole of the loop's low-pass.
const MAX_POLE: f64 = 0.97;
/// Dispersion all-passes in the loop.
const SECTIONS: f64 = 2.0;
/// A pipe's bore radius at Radius 1, as a fraction of its length: 0.6 of it
/// at both open ends lowers the note by a little under a semitone (R10).
const BORE: f64 = 0.0495;
/// How much louder than the excitation the loop gets at the most, when a
/// sine at a harmonic rings it for as long as it lasts (12 dB): the
/// excitation enters scaled by this much of the loop's distance from a gain
/// of 1, so one guide's output drives another's without either running away.
const RESONANCE_GAIN: f64 = 4.0;
/// The stiffness at Inharm 1 (R8).
const STIFFNESS: f64 = 0.001;

/// Waveguides for a number of voices, one per lane.
#[derive(Clone, Debug)]
pub struct WaveguideBank {
    rate: f64,
    voices: usize,
    stride: usize,
    kind: Kind,
    lowest: f64,
    mask: usize,
    /// Where this frame is written in the rings.
    cursor: usize,
    phase: usize,
    controls: Vec<Controls<CONTROLS>>,
    /// The loop's signal and the excitation's history, `rings × stride`.
    ring_s: Vec<f32>,
    ring_u: Vec<f32>,
    // Per voice (`stride` values): the loop.
    delay: Vec<u32>,
    thiran: Vec<f32>,
    disperse: Vec<f32>,
    pole: Vec<f32>,
    gain: Vec<f32>,
    sign: Vec<f32>,
    /// How much of the excitation the loop takes.
    drive: Vec<f32>,
    // Per voice: the three taps' delays in samples, the excitation's and the
    // two listening positions', and how much each moves per frame; and the
    // loop's length in samples.
    tap_delay: [Vec<f32>; 3],
    tap_step: [Vec<f32>; 3],
    length_samples: Vec<f32>,
    // Per voice: the filters' states.
    z_thiran: Vec<f32>,
    z_disperse: [Vec<f32>; 2],
    z_pole: Vec<f32>,
}

impl WaveguideBank {
    /// A bank of `voices` voices of `kind` at `sample_rate`, able to play
    /// down to `lowest_hz`. Allocates, and calls [`simd::level`]: make it off
    /// the audio thread.
    ///
    /// # Panics
    ///
    /// If `voices` is 0, or `sample_rate` or `lowest_hz` isn't positive.
    #[must_use]
    pub fn new(sample_rate: f32, voices: usize, kind: Kind, lowest_hz: f32) -> Self {
        assert!(voices > 0, "a bank has at least one voice");
        assert!(sample_rate > 0.0, "the sample rate must be positive");
        assert!(
            lowest_hz > 0.0 && lowest_hz < sample_rate / 1.5,
            "the lowest note must be positive and below the highest the guide plays"
        );
        let _ = simd::level();
        let rate = f64::from(sample_rate);
        let stride = voices.next_multiple_of(LANES);
        let rings = ((1.07 * rate / f64::from(lowest_hz)).ceil() as usize + 8).next_power_of_two();
        let zeros = || vec![0.0; stride];
        let mut bank = Self {
            rate,
            voices,
            stride,
            kind,
            lowest: f64::from(lowest_hz),
            mask: rings - 1,
            cursor: 0,
            phase: 0,
            controls: vec![Controls::new(controls_of(&WaveguideParams::default())); voices],
            ring_s: vec![0.0; rings * stride],
            ring_u: vec![0.0; rings * stride],
            delay: vec![1; stride],
            thiran: zeros(),
            disperse: zeros(),
            pole: zeros(),
            gain: zeros(),
            sign: vec![1.0; stride],
            drive: zeros(),
            tap_delay: std::array::from_fn(|_| vec![1.0; stride]),
            tap_step: std::array::from_fn(|_| zeros()),
            length_samples: zeros(),
            z_thiran: zeros(),
            z_disperse: std::array::from_fn(|_| zeros()),
            z_pole: zeros(),
        };
        for voice in 0..voices {
            bank.design(voice, true);
        }
        bank
    }

    /// How many voices it holds.
    #[must_use]
    pub fn voices(&self) -> usize {
        self.voices
    }

    /// How many values each frame takes per channel: the voices, padded to a
    /// multiple of 16.
    #[must_use]
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// Changes what the guides are, redesigning every voice.
    pub fn set_kind(&mut self, kind: Kind) {
        self.kind = kind;
        for voice in 0..self.voices {
            self.design(voice, true);
        }
    }

    /// Moves `voice` towards `params`: its delay through its fractional
    /// part, the rest smoothly (R17).
    ///
    /// # Panics
    ///
    /// If `voice` is out of range.
    pub fn set_params(&mut self, voice: usize, params: &WaveguideParams) {
        self.controls[voice].target = controls_of(params);
    }

    /// Sets `voice` to `params` with nothing moving and nothing ringing: the
    /// state a new note starts from.
    ///
    /// # Panics
    ///
    /// If `voice` is out of range.
    pub fn reset_voice(&mut self, voice: usize, params: &WaveguideParams) {
        self.controls[voice] = Controls::new(controls_of(params));
        self.clear(voice);
        self.delay[voice] = 0;
        self.design(voice, true);
    }

    /// Silences `voice`: its rings and its filters' states.
    fn clear(&mut self, voice: usize) {
        for row in 0..=self.mask {
            self.ring_s[row * self.stride + voice] = 0.0;
            self.ring_u[row * self.stride + voice] = 0.0;
        }
        for state in [&mut self.z_thiran, &mut self.z_pole]
            .into_iter()
            .chain(&mut self.z_disperse)
        {
            state[voice] = 0.0;
        }
    }

    /// Renders `input.len() / stride` frames: `input` is one excitation value
    /// per voice per frame; `output` takes the left channel's voices, then
    /// the right's, for each frame. The output depends only on the
    /// excitation and the settings, never on how blocks fall (R16).
    ///
    /// # Panics
    ///
    /// If `input` isn't a whole number of frames or `output` isn't twice as
    /// long.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.process_with(input, output, &mut ());
    }

    /// As [`process`](Self::process), with `interaction` driving each voice
    /// at its excitation position.
    ///
    /// # Panics
    ///
    /// As [`process`](Self::process).
    pub fn process_with<I: Interaction>(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        interaction: &mut I,
    ) {
        simd::dispatch(self.run(input, output, interaction));
    }

    /// As [`process`](Self::process), with lanes at `level`, for tests and
    /// benchmarks that compare paths. Gives `false`, leaving `output` alone,
    /// if this CPU can't run `level`.
    ///
    /// # Panics
    ///
    /// As [`process`](Self::process).
    pub fn process_at(&mut self, level: Level, input: &[f32], output: &mut [f32]) -> bool {
        simd::dispatch_at(level, self.run(input, output, &mut ())).is_ok()
    }

    fn run<'a, I: Interaction>(
        &'a mut self,
        input: &'a [f32],
        output: &'a mut [f32],
        interaction: &'a mut I,
    ) -> Run<'a, I> {
        assert_eq!(
            input.len() % self.stride,
            0,
            "the input holds a whole number of frames"
        );
        assert_eq!(
            output.len(),
            input.len() * 2,
            "the output holds two channels per frame"
        );
        Run {
            frames: input.len() / self.stride,
            bank: self,
            input,
            output,
            interaction,
        }
    }

    /// The control update: smoothing, redesigning what moved, flushing tiny
    /// state (R14, R17).
    fn tick(&mut self) {
        let alpha = smoothing(self.rate);
        let last = self.cursor.wrapping_sub(1) & self.mask;
        for voice in 0..self.voices {
            // A voice whose last value isn't a number, or has blown up, starts again from silence.
            if self.ring_s[last * self.stride + voice].abs() > BLOWN
                || self.ring_s[last * self.stride + voice].is_nan()
            {
                self.clear(voice);
            }
            let moved = self.controls[voice].step(alpha);
            if moved & LOOP_BITS != 0 {
                self.design(voice, false);
            } else {
                // Also when nothing moved: a tap that has arrived stops.
                self.place_taps(voice, false);
            }
        }
        for state in [&mut self.z_thiran, &mut self.z_pole]
            .into_iter()
            .chain(&mut self.z_disperse)
        {
            state.iter_mut().for_each(flush);
        }
    }

    /// Works out `voice`'s loop and taps from its current controls.
    fn design(&mut self, voice: usize, snap: bool) {
        let c = self.controls[voice].current;
        let hz = c[HZ].exp().clamp(self.lowest, self.rate / 1.5);
        let decay = c[DECAY].exp().clamp(1e-3, 60.0);
        let (mut material, inharm) = (c[2].clamp(0.0, 1.0), c[3].clamp(0.0, 1.0));
        let (opening, radius) = (c[4].clamp(0.0, 1.0), c[5].clamp(0.0, 1.0));
        let stiffness = inharm * STIFFNESS;

        let (negative, mut decay_scale, open_ends) = match self.kind {
            Kind::String => (false, 1.0, 0.0),
            Kind::Tube => (false, 1.0 + radius, 0.0),
            Kind::Pipe => (opening < 0.5, 1.0 + radius, 1.0 + opening),
        };
        if self.kind != Kind::String {
            material += (1.0 - material) * radius;
        }
        decay_scale *= decay;
        // Round trip, in samples, with the end correction (R10): 0.6 of the bore's radius at each open end.
        let length = self.rate / hz * (1.0 + 0.6 * BORE * radius * open_ends);
        // A pipe's far end reflects between closed (+1) and open (−1): the loop's sign and loss (R9).
        let reflection = if self.kind == Kind::Pipe {
            (2.0 * opening - 1.0).abs()
        } else {
            1.0
        };
        // The fundamental's angular frequency: half as high where the loop's sign flips.
        let fundamental = (if negative { PI } else { 2.0 * PI }) / length;
        // The reference harmonic for the damping and the dispersion: odd ones where the sign flips.
        let step = if negative { 2 } else { 1 };
        let harmonic = |limit: f64, cap: u32| {
            let mut k = 1;
            while f64::from(k + step) * fundamental <= limit && k + step <= cap {
                k += step;
            }
            k
        };

        // Loss: a gain and a one-pole low-pass, set so the fundamental falls 60 dB in `decay_scale` and a harmonic k in decay_scale × k^(−2(1 − m)).
        let at_fundamental = (-LN_1000 * length / (decay_scale * self.rate))
            .exp()
            .min(0.9995);
        let k_damp = harmonic(1.0, 8);
        let ratio = ((-2.0 * (1.0 - material) * f64::from(k_damp).ln()).exp()).recip();
        // |H(kω)|² ratio between the harmonic and the fundamental.
        let mut pole = 0.0;
        if k_damp > 1 && material < 1.0 {
            let falls = (-LN_1000 * length * (ratio - 1.0) / (decay_scale * self.rate)).exp();
            let q = falls * falls;
            let (c1, c2) = (fundamental.cos(), (f64::from(k_damp) * fundamental).cos());
            let q = q.max((1.0 - c1) / (1.0 - c2) * 1.01);
            if q < 1.0 {
                let b = c1 - q * c2;
                pole = (b - (b * b - (1.0 - q) * (1.0 - q)).max(0.0).sqrt()) / (1.0 - q);
            }
        }
        let budget = 0.25 * length;
        pole = pole.clamp(0.0, MAX_POLE.min(budget / (1.0 + budget)));
        let target = at_fundamental * reflection;
        let mut gain;
        loop {
            let co = fundamental.cos();
            gain = target * (1.0 - 2.0 * pole * co + pole * pole).sqrt() / (1.0 - pole);
            if gain <= LOOP_GAIN || pole < 1e-3 {
                break;
            }
            pole *= 0.9;
        }
        let gain = gain.min(LOOP_GAIN);

        // Dispersion: the all-pass coefficient that puts the reference harmonic where the stiff-string law does.
        let k_ref = harmonic(1.0, 6);
        let mut disperse = 0.0;
        if k_ref > step {
            let k = f64::from(k_ref);
            let there = k * fundamental * (1.0 + stiffness * k * k).sqrt();
            let want = length * ((1.0 + stiffness * k * k).sqrt().recip() - 1.0);
            let mismatch = |a: f64| {
                SECTIONS * (all_pass_delay(there, a) - all_pass_delay(fundamental, a))
                    + (low_pass_delay(there, pole) - low_pass_delay(fundamental, pole))
                    - want
            };
            let budget = (0.35 * length / SECTIONS).max(1.0);
            let lowest = -((budget - 1.0) / (budget + 1.0)).min(0.98);
            let (mut lo, mut hi) = (lowest, 0.9);
            if mismatch(lo) >= 0.0 {
                disperse = lo;
            } else if mismatch(hi) <= 0.0 {
                disperse = hi;
            } else {
                for _ in 0..40 {
                    let mid = (lo + hi) / 2.0;
                    if mismatch(mid) < 0.0 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                disperse = (lo + hi) / 2.0;
            }
        }

        // The delay that's left for the ring and the fractional all-pass.
        let fixed =
            low_pass_delay(fundamental, pole) + SECTIONS * all_pass_delay(fundamental, disperse);
        let left = (length - fixed).max(1.5);
        // Keep the ring's delay while the fractional part stays usable, so a moving control doesn't jump the read point; a low note sits in the middle of that range, where the all-pass is accurate however it moves. shortcut: a pitch move of more than a sample or two jumps the read point once per sample of it (a small click), where a cross-fade would hide it.
        let (centre, widest) = if fundamental < 0.3 {
            (1.0, 2.5)
        } else {
            (0.5, 1.5)
        };
        let kept = f64::from(self.delay[voice]);
        let whole = if kept >= 1.0 && (0.5..widest).contains(&(left - kept)) {
            kept
        } else {
            (left - centre).floor().max(1.0)
        };
        let (mut lo, mut hi) = (-0.6, 0.6);
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            // Phase delay falls as the coefficient rises.
            if all_pass_delay(fundamental, mid) > left - whole {
                lo = mid;
            } else {
                hi = mid;
            }
        }

        let at = voice;
        self.delay[at] = whole as u32;
        self.thiran[at] = ((lo + hi) / 2.0) as f32;
        self.disperse[at] = disperse as f32;
        self.pole[at] = pole as f32;
        self.gain[at] = (gain * (1.0 - pole)) as f32;
        self.sign[at] = if negative { -1.0 } else { 1.0 };
        self.drive[at] = (RESONANCE_GAIN * (1.0 - target)).min(1.0) as f32;
        self.length_samples[at] = length as f32;
        self.place_taps(voice, snap);
    }

    /// Sets the three taps' delays from the excitation and listening
    /// positions: `p` of the round trip each (R11). A tap moves to its new
    /// delay over the next [`CONTROL`] frames, a fraction of a sample at a
    /// time, unless `snap` puts it there at once.
    fn place_taps(&mut self, voice: usize, snap: bool) {
        let c = self.controls[voice].current;
        let length = f64::from(self.length_samples[voice]);
        // A tap reads four samples round its delay: at least one back.
        let most = (self.mask - 2) as f64;
        for (tap, position) in [c[6], c[7], c[8]].into_iter().enumerate() {
            let target = ((position.clamp(0.0, 0.5) * length).clamp(1.0, most)) as f32;
            if snap {
                self.tap_delay[tap][voice] = target;
                self.tap_step[tap][voice] = 0.0;
            } else {
                self.tap_step[tap][voice] = (target - self.tap_delay[tap][voice]) / CONTROL as f32;
            }
        }
    }
}

/// A first-order all-pass's phase delay at `w` radians per sample, for
/// coefficient `a`: (1 − a) / (1 + a) at zero.
fn all_pass_delay(w: f64, a: f64) -> f64 {
    1.0 - 2.0 / w * (a * w.sin()).atan2(1.0 + a * w.cos())
}

/// The phase delay of one pole `p` low-pass at `w` radians per sample.
fn low_pass_delay(w: f64, p: f64) -> f64 {
    (p * w.sin()).atan2(1.0 - p * w.cos()) / w
}

/// A voice's controls as the smoother holds them.
fn controls_of(params: &WaveguideParams) -> [f64; CONTROLS] {
    [
        sane(params.hz, 1.0, 1e6, 220.0).ln(),
        sane(params.decay, 1e-3, 60.0, 1.0).ln(),
        sane(params.material, 0.0, 1.0, 0.5),
        sane(params.inharm, 0.0, 1.0, 0.0),
        sane(params.opening, 0.0, 1.0, 1.0),
        sane(params.radius, 0.0, 1.0, 0.0),
        sane(params.excite, 0.0, 0.5, 0.25),
        sane(params.pickup_l, 0.0, 0.5, 0.25),
        sane(params.pickup_r, 0.0, 0.5, 0.25),
    ]
}

/// [`WaveguideBank::process`]'s kernel.
struct Run<'a, I: Interaction> {
    bank: &'a mut WaveguideBank,
    input: &'a [f32],
    output: &'a mut [f32],
    frames: usize,
    interaction: &'a mut I,
}

/// Each lane's voice's value `delay` frames back from `cursor` in the
/// interleaved ring.
#[inline(always)]
fn read<S: F32s>(
    ring: &[f32],
    mask: usize,
    stride: usize,
    cursor: usize,
    delay: &[u32],
    lane: usize,
) -> S {
    let mut at = [0.0; LANES];
    for (i, value) in at.iter_mut().enumerate().take(S::LANES) {
        let voice = lane + i;
        *value = ring[(cursor.wrapping_sub(delay[voice] as usize) & mask) * stride + voice];
    }
    S::load(&at)
}

/// A tap: each lane's voice's value `delay` samples back, between samples by
/// the four-point Lagrange interpolation, whose error grows as the fourth
/// power of the frequency, so a comb of it nulls the low harmonics deeply;
/// and it has no state, so a delay that moves every frame moves the sound
/// smoothly. Moves each delay on by its step.
#[inline(always)]
fn tap<S: F32s>(
    ring: &[f32],
    mask: usize,
    stride: usize,
    cursor: usize,
    delay: &mut [f32],
    step: &[f32],
    lane: usize,
) -> S {
    let mut out = [0.0; LANES];
    for (i, value) in out.iter_mut().enumerate().take(S::LANES) {
        let voice = lane + i;
        let at = delay[voice];
        // Steps can add up to a hair under the one sample a tap stops at.
        let read = at.max(1.0);
        let whole = read.floor();
        let f = read - whole;
        let back = whole as usize;
        let sample = |k: usize| ring[(cursor.wrapping_sub(k) & mask) * stride + voice];
        let (a, b, c, d) = (
            sample(back - 1),
            sample(back),
            sample(back + 1),
            sample(back + 2),
        );
        // Weights of the samples at -1, 0, 1 and 2 round the delay's whole part.
        let (wa, wb) = (
            -f * (f - 1.0) * (f - 2.0) / 6.0,
            (f + 1.0) * (f - 1.0) * (f - 2.0) / 2.0,
        );
        let (wc, wd) = (
            -(f + 1.0) * f * (f - 2.0) / 2.0,
            (f + 1.0) * f * (f - 1.0) / 6.0,
        );
        *value = wa * a + wb * b + wc * c + wd * d;
        delay[voice] = at + step[voice];
    }
    S::load(&out)
}

/// One first-order all-pass step on `lanes`: y = a x + z, z = x − a y.
#[inline(always)]
fn all_pass<S: F32s>(coefficient: &[f32], state: &mut [f32], lane: usize, x: S) -> S {
    let y = S::load(&coefficient[lane..]).mul_add(x, S::load(&state[lane..]));
    (-S::load(&coefficient[lane..]))
        .mul_add(y, x)
        .store(&mut state[lane..]);
    y
}

impl<I: Interaction> Kernel for Run<'_, I> {
    type Output = ();

    #[inline(always)]
    fn run<S: F32s>(self) {
        let Self {
            bank,
            input,
            output,
            frames,
            interaction,
        } = self;
        let (stride, mask) = (bank.stride, bank.mask);
        let groups = bank.voices.div_ceil(S::LANES);
        let mut done = 0;
        while done < frames {
            let chunk = (frames - done).min(CONTROL - bank.phase);
            for group in 0..groups {
                let lane = group * S::LANES;
                for frame in done..done + chunk {
                    let cursor = bank.cursor.wrapping_add(frame - done) & mask;
                    // The excitation is scaled by the loop's resonance; an interaction's force is not.
                    let mut u =
                        S::load(&input[frame * stride + lane..]) * S::load(&bank.drive[lane..]);
                    if I::ACTIVE {
                        let mut forced = [0.0; LANES];
                        u.store(&mut forced);
                        for (i, value) in forced.iter_mut().enumerate().take(S::LANES) {
                            let voice = lane + i;
                            if voice < bank.voices {
                                let force = interaction.force(voice, bank.velocity(voice, cursor));
                                *value += if force.is_finite() {
                                    force.clamp(-16.0, 16.0)
                                } else {
                                    0.0
                                };
                            }
                        }
                        u = S::load(&forced);
                    }
                    u.store(&mut bank.ring_u[cursor * stride + lane..]);
                    let excited = tap::<S>(
                        &bank.ring_u,
                        mask,
                        stride,
                        cursor,
                        &mut bank.tap_delay[0],
                        &bank.tap_step[0],
                        lane,
                    );

                    let wave = read::<S>(&bank.ring_s, mask, stride, cursor, &bank.delay, lane);
                    let wave = all_pass(&bank.thiran, &mut bank.z_thiran, lane, wave);
                    let wave = all_pass(&bank.disperse, &mut bank.z_disperse[0], lane, wave);
                    let wave = all_pass(&bank.disperse, &mut bank.z_disperse[1], lane, wave);
                    let low = S::load(&bank.pole[lane..]).mul_add(
                        S::load(&bank.z_pole[lane..]),
                        S::load(&bank.gain[lane..]) * wave,
                    );
                    low.store(&mut bank.z_pole[lane..]);
                    let wave = low * S::load(&bank.sign[lane..]) + (u - excited) * S::splat(0.5);
                    // A value below half of `TINY` becomes exactly zero and one up to `TINY` is faded out, keeping denormals out of the rings (R14).
                    let keep = wave.abs().mul_add(S::splat(2.0 / TINY), S::splat(-1.0));
                    let wave = wave * keep.max(S::splat(0.0)).min(S::splat(1.0));
                    wave.store(&mut bank.ring_s[cursor * stride + lane..]);

                    for (side, pick) in [1, 2].into_iter().enumerate() {
                        let heard = tap::<S>(
                            &bank.ring_s,
                            mask,
                            stride,
                            cursor,
                            &mut bank.tap_delay[pick],
                            &bank.tap_step[pick],
                            lane,
                        );
                        ((wave - heard) * S::splat(0.5))
                            .store(&mut output[(frame * 2 + side) * stride + lane..]);
                    }
                }
            }
            bank.cursor = bank.cursor.wrapping_add(chunk) & mask;
            bank.phase += chunk;
            done += chunk;
            if bank.phase == CONTROL {
                bank.phase = 0;
                bank.tick();
            }
        }
    }
}

impl WaveguideBank {
    /// The guide's velocity at `voice`'s excitation position, read the way
    /// the excitation's comb is written so a force proportional to it only
    /// ever damps: at every harmonic a round trip's delay is a whole number
    /// of cycles, so the comb's mirror image is the same comb read a round
    /// trip back with its taps swapped.
    fn velocity(&self, voice: usize, cursor: usize) -> f32 {
        let length = f64::from(self.length_samples[voice]);
        let back = self.controls[voice].current[6].clamp(0.0, 0.5) * length;
        let at = |delay: f64| {
            let whole = delay.floor();
            let fraction = (delay - whole) as f32;
            let slot =
                |d: usize| self.ring_s[(cursor.wrapping_sub(d) & self.mask) * self.stride + voice];
            slot(whole as usize) * (1.0 - fraction) + slot(whole as usize + 1) * fraction
        };
        self.sign[voice] * (at(length) - at(length - back))
    }
}
