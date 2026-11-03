//! Meld's filter slot (the meld spec, R40 to R58): seventeen types, each
//! with Frequency and two macros, run per voice and per frame.
//!
//! The first five (the two state-variable filters, MS2 low-pass and high-pass
//! and OSR) are the instruments' filter in the setups [`setup_of`] names. The
//! other twelve are Beatlayer's own structures, each one voice and frame at
//! a time: LP Crunch, LP Switched Res and Filther (nonlinear, bounded below
//! +12 dBFS), EQ Peak, EQ Notch, Phaser and Vowel (linear), Redux (reusing the
//! bit-crusher's quantiser and its low-pass), the two combs (a delay loop
//! tuned as a waveguide is) and the two resonators (the physical models' modal
//! bank, 32 modes, excited by the voice's signal). A change of type crossfades
//! over 5 ms, the old type running on under the new one (R56).

use std::f64::consts::{PI, TAU};

use bl_dsp::physical::{ModalBank, ModalParams, ModalTable, Quality, Shape};
use bl_model::Scale;

use crate::crusher::{low_pass, quantise};
use crate::instrument_filter::{InstrumentFilter, PerFrame, Setup, flush, limit, soft};
use crate::phaser_flanger::phaser_notch_freq;
use crate::sections::{Biquad, Coeffs};

/// How many filter types there are, and how many of them the instruments'
/// filter plays.
pub const TYPES: usize = 17;
const BUILT_IN: usize = 5;

const LP_CRUNCH: usize = 5;
const LP_SWITCHED_RES: usize = 6;
const FILTHER: usize = 7;
const EQ_PEAK: usize = 8;
const EQ_NOTCH: usize = 9;
const PHASER: usize = 10;
const REDUX: usize = 11;
const VOWEL: usize = 12;
const COMB_PLUS: usize = 13;
const COMB_MINUS: usize = 14;
const PLATE: usize = 15;
const MEMBRANE: usize = 16;

/// How long a change of type crossfades, in seconds (R56).
const FADE_SECONDS: f64 = 0.005;
/// The most Q a section has at full macro, and Butterworth's Q for one
/// section of a 12 dB response and the two of a 24 dB one.
const Q_MAX: f64 = 10.0;
const Q_BUTTERWORTH: f64 = std::f64::consts::FRAC_1_SQRT_2;
const Q_FIRST: f64 = 0.541_196_1;
const Q_SECOND: f64 = 1.306_563;
/// The most a macro's Drive adds, in decibels (R41).
const DRIVE_DB: f64 = 24.0;
/// The nonlinear types' output bound, just under +12 dBFS (R53).
const BOUND: f64 = 3.9;
/// LP Crunch's band-pass state softens above this, and the most of its output
/// it feeds back at full macros (R43).
const CRUNCH_SOFT: f64 = 1.5;
const CRUNCH_FEEDBACK: f64 = 0.6;
/// LP Switched Res's switching rate at Lofi 0 and at Lofi 100 %, in hertz (R44).
const SWITCH_HZ: (f64, f64) = (44_000.0, 500.0);
/// Phaser's stages, and the most feedback it has (R47).
const STAGES: usize = 6;
const PHASER_FEEDBACK: f64 = 0.95;
/// Peak's most boost and Q's range for the two EQ types (R46).
const BOOST_DB: f64 = 24.0;
const EQ_Q: (f64, f64) = (0.3, 12.0);
/// Vowel's Q range, and the weights of its three formants (R49).
const VOWEL_Q: (f64, f64) = (2.0, 30.0);
const VOWEL_WEIGHTS: [f64; 3] = [1.0, 0.8, 0.6];
/// A comb's most feedback, the highest Frequency it tunes within a cent
/// (as a share of the sample rate) and the lowest it delays (R50).
const COMB_FEEDBACK: f64 = 0.99;
const COMB_TOP: f64 = 0.3;
const COMB_BOTTOM_HZ: f64 = 20.0;
/// A comb's damping low-pass runs from this down to the other, in hertz.
const DAMP_HZ: (f64, f64) = (20_000.0, 500.0);
/// The resonators: the decay of the fundamental from Resonance's minimum to
/// its maximum, in seconds, and where the excitation and the listening point
/// sit on the plate or membrane (R51).
const DECAY_SECONDS: (f64, f64) = (0.01, 10.0);
const HIT: f32 = 0.35;
const LISTEN: f32 = 0.6;
/// A resonator's input is scaled by the inverse of the square root of the
/// share of its fundamental that falls away each frame, so a signal of any
/// level and any Resonance rings at about its own level, as noise through a
/// bank of narrow lines would not.
const MAKEUP: f64 = 1.0;
/// The plate's Ratio range (R51).
const RATIO: (f64, f64) = (0.25, 4.0);
/// The highest Frequency a filter's design follows, as a share of the rate.
const TOP: f64 = 0.45;

/// The vowels' first three formants in hertz, A, E, I, O, U: the average
/// adult male values of Peterson and Barney (1952), as Analog's first two.
const VOWELS: [[f64; 3]; 5] = [
    [730.0, 1090.0, 2440.0],
    [530.0, 1840.0, 2480.0],
    [270.0, 2290.0, 3010.0],
    [570.0, 840.0, 2410.0],
    [300.0, 870.0, 2240.0],
];

/// The instruments' filter's setup for filter type `ty` below five: SVF 12dB
/// and SVF 24dB on its Clean circuit morphing, MS2 as a 12 dB low-pass and
/// high-pass, OSR as a 12 dB band-pass (R12, R42).
fn setup_of(ty: usize) -> Setup {
    match ty {
        0 => Setup::new(4, false, 0),
        1 => Setup::new(4, true, 0),
        2 => Setup::new(0, false, 2),
        3 => Setup::new(1, false, 2),
        _ => Setup::new(2, false, 1),
    }
}

/// What a block's filtering reads, for each frame (R55): the instruments'
/// filter's values, Frequency in hertz and each macro from 0 to 1, shared by
/// every voice, and the scale the resonators snap to, if they do (R52).
#[derive(Clone, Copy)]
pub struct Controls<'a> {
    pub frame: PerFrame<'a>,
    pub hz: &'a [f32],
    pub macro_1: &'a [f32],
    pub macro_2: &'a [f32],
    pub scale: Option<Scale>,
}

/// The filter of every voice slot of an engine.
pub struct MeldFilter {
    stages: [Stage; 2],
    current: usize,
    ty: usize,
    fade_left: usize,
    fade_frames: usize,
    played: bool,
    stride: usize,
    work: Vec<f32>,
}

impl MeldFilter {
    /// A filter for `slots` voices at `rate`, for blocks of up to
    /// `max_frames`. Allocates, so make it off the audio thread.
    #[must_use]
    pub fn new(rate: f32, slots: usize, max_frames: usize) -> Self {
        let stages = [
            Stage::new(rate, slots, max_frames),
            Stage::new(rate, slots, max_frames),
        ];
        let stride = stages[0].inst.stride();
        Self {
            stages,
            current: 0,
            ty: 0,
            fade_left: 0,
            fade_frames: ((f64::from(rate) * FADE_SECONDS) as usize).max(1),
            played: false,
            stride,
            work: vec![0.0; max_frames.max(1) * 2 * stride],
        }
    }

    /// How many values a frame of the per-voice buffers holds: the layout of
    /// `audio` in [`process`](Self::process).
    #[must_use]
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// Moves to filter type `ty`: the type that played fades out as the new
    /// one starts from silence (R56). Nothing happens if it is playing.
    pub fn set_type(&mut self, ty: usize) {
        let ty = ty.min(TYPES - 1);
        if ty == self.ty && self.played {
            return;
        }
        if self.played && ty != self.ty {
            self.current = 1 - self.current;
            self.fade_left = self.fade_frames;
        }
        self.ty = ty;
        self.stages[self.current].restart(ty);
    }

    /// Starts slot `slot`'s voice afresh: its state clears and its stepping
    /// counts from its next frame.
    pub fn reset_voice(&mut self, slot: usize) {
        for stage in &mut self.stages {
            stage.reset_voice(slot);
        }
    }

    /// Filters `frames` frames of `audio` (per frame, then per channel, one
    /// value per slot; the voices' signal is the first channel) for the first
    /// `hi` slots, in place.
    pub fn process(&mut self, hi: usize, frames: usize, c: &Controls<'_>, audio: &mut [f32]) {
        self.played = true;
        let used = frames * 2 * self.stride;
        let fading = self.fade_left > 0;
        if fading {
            self.work[..used].copy_from_slice(&audio[..used]);
            self.stages[1 - self.current].run(hi, frames, c, self.stride, &mut self.work[..used]);
        }
        self.stages[self.current].run(hi, frames, c, self.stride, &mut audio[..used]);
        if fading {
            let stride = self.stride;
            for frame in 0..frames {
                let left = self.fade_left.saturating_sub(frame);
                let old = left as f32 / self.fade_frames as f32;
                let at = frame * 2 * stride;
                let span = at..at + 2 * stride;
                for (new, old_out) in audio[span.clone()].iter_mut().zip(&self.work[span]) {
                    *new += (old_out - *new) * old;
                }
            }
            self.fade_left = self.fade_left.saturating_sub(frames);
        }
    }
}

/// One filter type's structure, playing or fading out.
struct Stage {
    ty: usize,
    inst: InstrumentFilter,
    extra: Extra,
}

impl Stage {
    fn new(rate: f32, slots: usize, max_frames: usize) -> Self {
        Self {
            ty: 0,
            inst: InstrumentFilter::new(rate, slots, max_frames),
            extra: Extra::new(f64::from(rate), slots, max_frames),
        }
    }

    fn restart(&mut self, ty: usize) {
        self.ty = ty;
        if ty < BUILT_IN {
            self.inst.restart(setup_of(ty));
        } else {
            self.extra.restart(ty);
        }
    }

    fn reset_voice(&mut self, slot: usize) {
        if self.ty < BUILT_IN {
            self.inst.reset_voice(slot, 0);
        } else {
            self.extra.reset_voice(slot);
        }
    }

    fn run(
        &mut self,
        hi: usize,
        frames: usize,
        c: &Controls<'_>,
        stride: usize,
        audio: &mut [f32],
    ) {
        if self.ty < BUILT_IN {
            self.inst.process(hi, frames, &c.frame, audio);
        } else {
            self.extra.run(self.ty, hi, frames, c, stride, audio);
        }
    }
}

/// What a frame's filtering needs, worked out from Frequency and the macros.
#[derive(Clone, Copy, Default)]
struct Coef {
    /// The prewarped cutoff, `tan(pi fc / rate)`.
    g: f64,
    /// Each section's Q.
    q: [f64; 2],
    /// The drive's gain into the filter, and the feedback or damping the type
    /// uses.
    gain: f64,
    fb: f64,
    /// A biquad's `[b0, b1, b2, a1, a2]`, or the stages' all-pass
    /// coefficients, or the formants' prewarped cutoffs.
    bq: [f64; 5],
    stage: [f64; STAGES],
    formant: [f64; 3],
    /// Switched Res's and Redux's clock step, in ticks a frame, Redux's bit
    /// depth and Lofi.
    step: f64,
    bits: u32,
    lofi: f64,
    /// A comb's delay in whole frames, its all-pass and low-pass coefficients,
    /// its feedback's sign and its gain on the input.
    delay: usize,
    ap: f64,
    lp: f64,
    sign: f64,
    norm: f64,
}

/// `q0` taken up to Q_MAX by `r`, as the instruments' filter does.
fn q_of(q0: f64, r: f64) -> f64 {
    q0 * (Q_MAX / q0).powf(r)
}

fn log_lerp(range: (f64, f64), t: f64) -> f64 {
    range.0 * (range.1 / range.0).powf(t)
}

fn tan_of(hz: f64, rate: f64) -> f64 {
    (PI * hz.clamp(1.0, rate * TOP) / rate).tan()
}

/// A first-order all-pass's coefficient for a 90° turn at `hz`.
fn stage_of(hz: f64, rate: f64) -> f64 {
    let t = tan_of(hz.clamp(20.0, rate * TOP), rate);
    (t - 1.0) / (t + 1.0)
}

/// A state-variable section's low-pass, band-pass (peak gain 1) and
/// high-pass at prewarped cutoff `g` and Q `q`, its two states in `s`.
#[inline]
fn svf(g: f64, q: f64, s: &mut [f64], x: f64) -> [f64; 3] {
    let k = 1.0 / q;
    let a1 = 1.0 / (1.0 + g * (g + k));
    let (a2, a3) = (g * a1, g * g * a1);
    let v3 = x - s[1];
    let v1 = a1 * s[0] + a2 * v3;
    let v2 = s[1] + a2 * s[0] + a3 * v3;
    s[0] = 2.0 * v1 - s[0];
    s[1] = 2.0 * v2 - s[1];
    [v2, k * v1, x - k * v1 - v2]
}

impl Coef {
    /// The coefficients of type `ty` for Frequency `hz` and macros `m1`
    /// and `m2`.
    fn of(ty: usize, hz: f64, m1: f64, m2: f64, rate: f64) -> Self {
        let sane = |x: f64, lo: f64, hi: f64| if x.is_finite() { x.clamp(lo, hi) } else { lo };
        let hz = sane(hz, 1.0, 1e6);
        let (m1, m2) = (sane(m1, 0.0, 1.0), sane(m2, 0.0, 1.0));
        let mut c = Self {
            g: tan_of(hz, rate),
            gain: 10f64.powf(m2 * DRIVE_DB / 20.0),
            ..Self::default()
        };
        match ty {
            LP_CRUNCH => {
                c.q[0] = q_of(Q_BUTTERWORTH, m1);
                c.fb = CRUNCH_FEEDBACK * 0.5 * (m1 + m2);
            }
            LP_SWITCHED_RES => {
                let switching = log_lerp(SWITCH_HZ, m2);
                c.step = (switching / rate).min(1.0);
                c.g = tan_of(hz.min(TOP * switching), switching.min(rate));
                c.q[0] = q_of(Q_BUTTERWORTH, m1);
            }
            FILTHER => c.q = [Q_FIRST, q_of(Q_SECOND, m1)],
            EQ_PEAK | EQ_NOTCH => {
                let w0 = TAU * hz.clamp(20.0, rate * TOP) / rate;
                let (sin, cos) = w0.sin_cos();
                let alpha = sin / (2.0 * log_lerp(EQ_Q, m2));
                // The analogue prototype's numerator and denominator in
                // `alpha`, then the bilinear transform's: peak boosts by
                // `a`, notch cuts to `a` at the centre.
                let (num, den) = if ty == EQ_PEAK {
                    let a = 10f64.powf(m1 * BOOST_DB / 40.0);
                    (alpha * a, alpha / a)
                } else {
                    (alpha * (1.0 - m1), alpha)
                };
                let a0 = 1.0 + den;
                c.bq = [
                    (1.0 + num) / a0,
                    -2.0 * cos / a0,
                    (1.0 - num) / a0,
                    -2.0 * cos / a0,
                    (1.0 - den) / a0,
                ];
            }
            PHASER => {
                for (k, stage) in c.stage.iter_mut().enumerate() {
                    *stage = stage_of(phaser_notch_freq(hz, STAGES, m2 * 100.0, k), rate);
                }
                c.fb = PHASER_FEEDBACK * m1;
            }
            REDUX => {
                c.step = (hz / rate).min(1.0);
                c.bits = (16.0 - 15.0 * m1).round() as u32;
                c.lofi = m2;
            }
            VOWEL => {
                let place = 4.0 * m2;
                let first = (place as usize).min(3);
                let share = place - first as f64;
                for (j, formant) in c.formant.iter_mut().enumerate() {
                    let at = VOWELS[first][j] * (1.0 - share) + VOWELS[first + 1][j] * share;
                    *formant = tan_of(at * hz / 1_000.0, rate);
                }
                c.q[0] = log_lerp(VOWEL_Q, m1);
            }
            COMB_PLUS | COMB_MINUS => {
                let f = hz.clamp(COMB_BOTTOM_HZ, rate * COMB_TOP);
                let w0 = TAU * f / rate;
                let damp = log_lerp((DAMP_HZ.0, DAMP_HZ.1), m2).min(TOP * rate);
                c.lp = if m2 <= 0.0 {
                    1.0
                } else {
                    1.0 - (-TAU * damp / rate).exp()
                };
                // The loop's whole delay is a period of `f`; the low-pass's
                // phase delay at `f` comes off it, and the all-pass takes the
                // fraction left over (R50, physical models R8).
                let b = 1.0 - c.lp;
                let lp_delay = (b * w0.sin()).atan2(1.0 - b * w0.cos()) / w0;
                let whole = rate / f - lp_delay;
                c.delay = (whole - 0.5).floor().max(1.0) as usize;
                let t = (((whole - c.delay as f64) * w0 / 2.0).tan() / (w0 / 2.0).tan()).max(0.0);
                c.ap = (1.0 - t) / (1.0 + t);
                c.fb = COMB_FEEDBACK * m1;
                c.sign = if ty == COMB_PLUS { 1.0 } else { -1.0 };
                c.norm = (1.0 - c.fb * c.fb).sqrt();
            }
            _ => {}
        }
        c
    }
}

/// The filter types that aren't the instruments': the state of each voice,
/// and what the combs and the resonators keep.
struct Extra {
    rate: f64,
    slots: usize,
    /// Each voice's state: a type's integrators, clocks and held values.
    state: Vec<[f64; 8]>,
    /// Slots that started since the last block.
    fresh: Vec<u16>,
    coef: Coef,
    seen: [f32; 3],
    /// Redux's two low-pass sections for each voice, and their coefficients.
    pre: Vec<[Biquad; 2]>,
    pre_coeffs: [Coeffs; 2],
    /// The combs' delay lines, a ring per voice with one write place.
    ring: Vec<f32>,
    ring_len: usize,
    place: usize,
    /// The resonators: a bank each, what they are set to, the scale they
    /// last snapped to, the frames they have run and the block's excitation
    /// and output.
    plate: ModalBank,
    membrane: ModalBank,
    params: ModalParams,
    scale: Option<Scale>,
    applied: bool,
    makeup: f32,
    frames_run: u64,
    modal_in: Vec<f32>,
    modal_out: Vec<f32>,
}

impl Extra {
    fn new(rate: f64, slots: usize, max_frames: usize) -> Self {
        let bank = |shape| {
            let mut bank = ModalBank::new(rate as f32, slots);
            drop(bank.set_table(ModalTable::new(shape, Quality::Medium, 1.0, 0.0)));
            bank
        };
        let modal_stride = slots.next_multiple_of(16);
        let ring_len = (rate / COMB_BOTTOM_HZ).ceil() as usize + 8;
        Self {
            rate,
            slots,
            state: vec![[0.0; 8]; slots],
            fresh: Vec::with_capacity(slots),
            coef: Coef::default(),
            seen: [f32::NAN; 3],
            pre: vec![[Biquad::default(); 2]; slots],
            pre_coeffs: [Coeffs::default(); 2],
            ring: vec![0.0; slots * ring_len],
            ring_len,
            place: 0,
            plate: bank(Shape::Plate),
            membrane: bank(Shape::CircularMembrane),
            params: ModalParams::default(),
            scale: None,
            applied: false,
            makeup: 1.0,
            frames_run: 0,
            modal_in: vec![0.0; max_frames.max(1) * modal_stride],
            modal_out: vec![0.0; max_frames.max(1) * 2 * modal_stride],
        }
    }

    /// Starts afresh as type `ty`, every voice from silence.
    fn restart(&mut self, ty: usize) {
        self.state.fill([0.0; 8]);
        self.fresh.clear();
        self.seen = [f32::NAN; 3];
        self.place = 0;
        self.frames_run = 0;
        self.applied = false;
        self.scale = None;
        for pre in &mut self.pre {
            *pre = [Biquad::default(); 2];
        }
        if matches!(ty, COMB_PLUS | COMB_MINUS) {
            self.ring.fill(0.0);
        }
        if matches!(ty, PLATE | MEMBRANE) {
            for lane in 0..self.slots {
                self.plate.reset_voice(lane, &self.params);
                self.membrane.reset_voice(lane, &self.params);
            }
        }
    }

    fn reset_voice(&mut self, slot: usize) {
        if !self.fresh.contains(&(slot as u16)) {
            self.fresh.push(slot as u16);
        }
    }

    /// Clears the voices that started since the last block.
    fn start_voices(&mut self, ty: usize) {
        for &lane in &self.fresh {
            let lane = usize::from(lane);
            self.state[lane] = [0.0; 8];
            self.pre[lane] = [Biquad::default(); 2];
            for (biquad, c) in self.pre[lane].iter_mut().zip(self.pre_coeffs) {
                biquad.c = c;
            }
            if matches!(ty, COMB_PLUS | COMB_MINUS) {
                self.ring[lane * self.ring_len..(lane + 1) * self.ring_len].fill(0.0);
            }
            if ty == PLATE {
                self.plate.reset_voice(lane, &self.params);
            } else if ty == MEMBRANE {
                self.membrane.reset_voice(lane, &self.params);
            }
        }
        self.fresh.clear();
    }

    fn run(
        &mut self,
        ty: usize,
        hi: usize,
        frames: usize,
        c: &Controls<'_>,
        stride: usize,
        audio: &mut [f32],
    ) {
        self.start_voices(ty);
        if matches!(ty, PLATE | MEMBRANE) {
            self.resonate(ty, hi, frames, c, stride, audio);
        } else {
            self.run_frames(ty, hi, frames, c, stride, audio);
        }
    }

    /// The types that run a frame and a voice at a time.
    fn run_frames(
        &mut self,
        ty: usize,
        hi: usize,
        frames: usize,
        c: &Controls<'_>,
        stride: usize,
        audio: &mut [f32],
    ) {
        for frame in 0..frames {
            let now = [c.hz[frame], c.macro_1[frame], c.macro_2[frame]];
            if now != self.seen {
                self.seen = now;
                self.coef = Coef::of(
                    ty,
                    f64::from(now[0]),
                    f64::from(now[1]),
                    f64::from(now[2]),
                    self.rate,
                );
                if ty == REDUX {
                    let half = (f64::from(now[0]) / 2.0).clamp(1.0, self.rate);
                    self.pre_coeffs = low_pass(half, self.rate);
                    for pre in &mut self.pre[..hi] {
                        for (biquad, c) in pre.iter_mut().zip(self.pre_coeffs) {
                            biquad.c = c;
                        }
                    }
                }
            }
            let coef = &self.coef;
            for lane in 0..hi {
                let at = frame * 2 * stride + lane;
                let x = f64::from(audio[at]);
                let s = &mut self.state[lane];
                let y = match ty {
                    LP_CRUNCH => crunch(coef, s, x),
                    LP_SWITCHED_RES => switched(coef, s, x),
                    FILTHER => filther(coef, s, x),
                    EQ_PEAK | EQ_NOTCH => biquad(&coef.bq, s, x),
                    PHASER => phaser(coef, s, x),
                    REDUX => redux(coef, s, &mut self.pre[lane], x),
                    VOWEL => vowel(coef, s, x),
                    _ => {
                        let ring = &mut self.ring[lane * self.ring_len..(lane + 1) * self.ring_len];
                        comb(coef, s, ring, self.place, x)
                    }
                };
                audio[at] = if y.is_finite() { y as f32 } else { 0.0 };
            }
            self.place = (self.place + 1) % self.ring_len;
        }
        self.frames_run += frames as u64;
    }

    /// The resonators: the voices' signal through the modal bank, its
    /// controls set on the bank's grid of 32 frames and, when the scale
    /// changes, on the frame it does (R52).
    fn resonate(
        &mut self,
        ty: usize,
        hi: usize,
        frames: usize,
        c: &Controls<'_>,
        stride: usize,
        audio: &mut [f32],
    ) {
        let plate = ty == PLATE;
        let bank = if plate {
            &mut self.plate
        } else {
            &mut self.membrane
        };
        let modal_stride = bank.stride();
        self.modal_in[..frames * modal_stride].fill(0.0);
        for frame in 0..frames {
            for lane in 0..hi {
                self.modal_in[frame * modal_stride + lane] = audio[frame * 2 * stride + lane];
            }
        }
        let scale_moved = self.scale != c.scale;
        self.scale = c.scale;
        let mut start = 0;
        while start < frames {
            let phase =
                ((self.frames_run + start as u64) % bl_dsp::physical::CONTROL as u64) as usize;
            let end = (start + bl_dsp::physical::CONTROL - phase).min(frames);
            if phase == 0 || !self.applied || (start == 0 && scale_moved) {
                self.applied = true;
                let (m1, m2) = (f64::from(c.macro_1[start]), f64::from(c.macro_2[start]));
                let mut hz = f64::from(c.hz[start]).clamp(1.0, 1e6);
                if let Some(scale) = &c.scale {
                    hz = snapped(scale, hz);
                }
                // Plate's Ratio is the plate's sides; Membrane's Damping is
                // the physical models' Material turned round (R51).
                let ratio = log_lerp(RATIO, m2) as f32;
                self.params = ModalParams {
                    hz: hz as f32,
                    decay: log_lerp(DECAY_SECONDS, m1) as f32,
                    material: if plate { 0.5 } else { (1.0 - m2) as f32 },
                    hit: HIT,
                    pos_l: LISTEN,
                    pos_r: LISTEN,
                    ..ModalParams::default()
                };
                let falls = 6.907_755 / (f64::from(self.params.decay) * self.rate);
                self.makeup = (MAKEUP / falls.sqrt()) as f32;
                if plate {
                    bank.rebuild_table(Shape::Plate, Quality::Medium, ratio, 0.0);
                }
                for lane in 0..bank.voices() {
                    bank.set_params(lane, &self.params);
                }
            }
            for x in &mut self.modal_in[start * modal_stride..end * modal_stride] {
                *x *= self.makeup;
            }
            bank.process(
                &self.modal_in[start * modal_stride..end * modal_stride],
                &mut self.modal_out[start * 2 * modal_stride..end * 2 * modal_stride],
            );
            start = end;
        }
        for frame in 0..frames {
            for lane in 0..hi {
                let wet = f64::from(self.modal_out[frame * 2 * modal_stride + lane]);
                audio[frame * 2 * stride + lane] = limit(wet, BOUND) as f32;
            }
        }
        self.frames_run += frames as u64;
    }
}

/// `hz` moved to the nearest note of `scale`: its twelve-step pitch (the
/// scale is chromatic under a tuning, scales R15).
fn snapped(scale: &Scale, hz: f64) -> f64 {
    let key = (69.0 + 12.0 * (hz / 440.0).log2())
        .round()
        .clamp(0.0, 127.0) as u8;
    440.0 * ((f64::from(scale.nearest(key)) - 69.0) / 12.0).exp2()
}

/// LP Crunch (R43): a 12 dB low-pass whose band-pass state is softened, with
/// its own distorted output fed back into its input.
fn crunch(c: &Coef, s: &mut [f64; 8], x: f64) -> f64 {
    let input = x * c.gain + c.fb * s[2];
    let k = 1.0 / c.q[0];
    let a1 = 1.0 / (1.0 + c.g * (c.g + k));
    let (a2, a3) = (c.g * a1, c.g * c.g * a1);
    let v3 = input - s[1];
    let v1 = a1 * s[0] + a2 * v3;
    let v2 = s[1] + a2 * s[0] + a3 * v3;
    s[0] = flush(soft(2.0 * v1 - s[0], CRUNCH_SOFT));
    s[1] = flush(2.0 * v2 - s[1]);
    s[2] = v2.tanh();
    limit(v2, BOUND)
}

/// LP Switched Res (R44): a 12 dB low-pass that updates only when its clock
/// ticks, its output held between. The clock counts from the voice's start.
fn switched(c: &Coef, s: &mut [f64; 8], x: f64) -> f64 {
    s[2] += c.step;
    if s[2] >= 1.0 {
        s[2] -= 1.0;
        let [low, ..] = svf(c.g, c.q[0], &mut s[..2], x);
        s[0] = flush(s[0]);
        s[1] = flush(s[1]);
        s[3] = low;
    }
    limit(s[3], BOUND)
}

/// Filther (R45): a hard clip on the input, two sections of a 24 dB low-pass,
/// a tanh on the output.
fn filther(c: &Coef, s: &mut [f64; 8], x: f64) -> f64 {
    let mut y = (x * c.gain).clamp(-1.0, 1.0);
    for (section, q) in s.chunks_mut(2).zip(c.q) {
        [y, ..] = svf(c.g, q, section, y);
        section[0] = flush(section[0]);
        section[1] = flush(section[1]);
    }
    y.tanh()
}

/// A biquad in transposed direct form II (R46).
fn biquad(bq: &[f64; 5], s: &mut [f64; 8], x: f64) -> f64 {
    let y = flush(bq[0] * x + s[0]);
    s[0] = flush(bq[1] * x - bq[3] * y + s[1]);
    s[1] = flush(bq[2] * x - bq[4] * y);
    y
}

/// Phaser (R47): six first-order all-passes with the output fed back
/// inverted, `A / (1 + F A)`, the feedback's own sample solved for.
fn phaser(c: &Coef, s: &mut [f64; 8], x: f64) -> f64 {
    let (mut held, mut through) = (0.0, 1.0);
    for (stage, z) in c.stage.iter().zip(s.iter()) {
        held = stage * held + z;
        through *= stage;
    }
    let out = (through * x + held) / (1.0 + c.fb * through);
    let mut v = x - c.fb * out;
    for (stage, z) in c.stage.iter().zip(s.iter_mut()) {
        let y = stage * v + *z;
        *z = flush(v - stage * y);
        v = y;
    }
    v
}

/// Redux (R48): the signal through a 24 dB low-pass at half the rate, mixed
/// with the unfiltered by Lofi, held at the rate and quantised to the bit
/// depth Crush gives, with the bit-crusher's quantiser.
fn redux(c: &Coef, s: &mut [f64; 8], pre: &mut [Biquad; 2], x: f64) -> f64 {
    let mut held = x as f32;
    if c.step < 1.0 {
        let filtered = pre.iter_mut().fold(x as f32, |y, stage| stage.process(y));
        held += (filtered - held) * (1.0 - c.lofi as f32);
        s[0] += c.step;
        if s[0] >= 1.0 {
            s[0] = (s[0] - 1.0).min(1.0);
            s[1] = f64::from(held);
        }
    } else {
        s[1] = f64::from(held);
    }
    f64::from(quantise(s[1] as f32, c.bits.clamp(1, 16), 1.0, 0.0))
}

/// Vowel (R49): three band-passes at the formants, weighted and summed.
fn vowel(c: &Coef, s: &mut [f64; 8], x: f64) -> f64 {
    let mut sum = 0.0;
    for (j, section) in s.chunks_mut(2).take(3).enumerate() {
        let [_, band, _] = svf(c.formant[j], c.q[0], section, x);
        section[0] = flush(section[0]);
        section[1] = flush(section[1]);
        sum += VOWEL_WEIGHTS[j] * band;
    }
    sum
}

/// Comb + and Comb − (R50): `w[n] = norm x[n] + sign g LP(w[n − D])`, the
/// delay a whole number of frames in `ring` and a first-order Thiran
/// all-pass tuned at Frequency, the low-pass's own delay taken off.
fn comb(c: &Coef, s: &mut [f64; 8], ring: &mut [f32], place: usize, x: f64) -> f64 {
    let len = ring.len();
    let delayed = f64::from(ring[(place + len - c.delay % len) % len]);
    let shifted = c.ap * delayed + s[0];
    s[0] = flush(delayed - c.ap * shifted);
    s[1] = flush(s[1] + c.lp * (shifted - s[1]));
    let w = c.norm * x + c.sign * c.fb * s[1];
    ring[place] = w as f32;
    w
}
