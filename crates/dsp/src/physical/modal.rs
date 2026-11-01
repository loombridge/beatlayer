//! A bank of modal resonators: per voice, a set of decaying sine partials
//! (physical models R1 to R7, R12 to R17).
//!
//! Each partial is a damped "magic circle" (Smith, 1985): its frequency
//! comes from `ε = 2 sin(ω / 2)`, so it's accurate to the precision of `f32`
//! at every pitch and rate, where a rotation by `cos ω` would lose a cent at
//! 40 Hz and 192 kHz. A voice's partials sit in rows of the arrays, a lane
//! per voice.

use lspk_dsp::simd::{self, F32s, Kernel, Level};

use super::shapes::{MAX_PARTIALS, ModalTable, Quality, Shape};
use super::{CONTROL, Controls, LANES, LN_1000, flush, sane, smoothing};

/// How much louder than the excitation a partial gets at the most, when a
/// sine at its frequency rings it for as long as it lasts (12 dB). The
/// excitation enters each partial scaled by this much of its pole's distance
/// from the unit circle, so a resonator's output drives another's without
/// either running away.
const RESONANCE_GAIN: f64 = 4.0;

/// A partial whose frequency reaches this fraction of the sample rate is
/// left out (R3).
const NYQUIST: f64 = 0.45;

const TUNING: u8 = 1;
const DECAY: u8 = 2;
const GAIN: u8 = 4;
const ALL: u8 = TUNING | DECAY | GAIN;

/// What a modal voice is set to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModalParams {
    /// The fundamental, in hertz.
    pub hz: f32,
    /// Seconds for the fundamental to fall 60 dB, 0.001 to 60 (R5).
    pub decay: f32,
    /// 0 for wood or rubber, 1 for glass or metal (R5).
    pub material: f32,
    /// −1 to 1, weighting upper partials (R6).
    pub brightness: f32,
    /// Where it's struck, 0 (the centre) to 1 (the edge) (R7).
    pub hit: f32,
    /// Where the left channel listens (R7).
    pub pos_l: f32,
    /// Where the right channel listens (R7).
    pub pos_r: f32,
}

impl Default for ModalParams {
    fn default() -> Self {
        Self {
            hz: 220.0,
            decay: 1.0,
            material: 0.5,
            brightness: 0.0,
            hit: 0.0,
            pos_l: 0.0,
            pos_r: 0.0,
        }
    }
}

/// Resonators for a number of voices, one per lane.
#[derive(Clone, Debug)]
pub struct ModalBank {
    rate: f64,
    voices: usize,
    stride: usize,
    table: ModalTable,
    table_changed: bool,
    hz: Vec<f64>,
    /// Per voice: ln decay, material, brightness, hit, left and right.
    controls: Vec<Controls<6>>,
    count: Vec<usize>,
    dirty: Vec<u8>,
    /// Frames since the last control update.
    phase: usize,
    /// The most partials any voice has.
    active: usize,
    /// Per partial and voice, `MAX_PARTIALS × stride` values each.
    eps: Vec<f32>,
    pole: Vec<f32>,
    inject: Vec<f32>,
    gain_l: Vec<f32>,
    gain_r: Vec<f32>,
    x: Vec<f32>,
    y: Vec<f32>,
}

impl ModalBank {
    /// A bank of `voices` voices at `sample_rate`, with a Beam of Medium
    /// quality. Allocates, and calls [`simd::level`]: make it off the audio
    /// thread.
    ///
    /// # Panics
    ///
    /// If `voices` is 0 or `sample_rate` isn't positive.
    #[must_use]
    pub fn new(sample_rate: f32, voices: usize) -> Self {
        assert!(voices > 0, "a bank has at least one voice");
        assert!(sample_rate > 0.0, "the sample rate must be positive");
        let _ = simd::level();
        let stride = voices.next_multiple_of(LANES);
        let defaults = ModalParams::default();
        let size = MAX_PARTIALS * stride;
        let mut bank = Self {
            rate: f64::from(sample_rate),
            voices,
            stride,
            table: ModalTable::new(Shape::Beam, Quality::Medium, 1.0, 0.0),
            table_changed: false,
            hz: vec![f64::from(defaults.hz); voices],
            controls: vec![Controls::new(controls_of(&defaults)); voices],
            count: vec![0; voices],
            dirty: vec![ALL; voices],
            phase: 0,
            active: 0,
            eps: vec![0.0; size],
            pole: vec![0.0; size],
            inject: vec![0.0; size],
            gain_l: vec![0.0; size],
            gain_r: vec![0.0; size],
            x: vec![0.0; size],
            y: vec![0.0; size],
        };
        for voice in 0..voices {
            bank.refresh(voice);
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

    /// Swaps in `table` for every voice (R12) and gives back the one it
    /// replaced, to be dropped off the audio thread. Nothing allocates or
    /// frees here.
    pub fn set_table(&mut self, table: ModalTable) -> ModalTable {
        self.table_changed = true;
        std::mem::replace(&mut self.table, table)
    }

    /// Makes the bank's table what [`ModalTable::new`] would, in place and
    /// without allocating ([`ModalTable::rebuild`]), for every voice at the
    /// next control update.
    pub fn rebuild_table(
        &mut self,
        shape: Shape,
        quality: Quality,
        aspect: f32,
        inharmonicity: f32,
    ) {
        self.table_changed = true;
        self.table.rebuild(shape, quality, aspect, inharmonicity);
    }

    /// Moves `voice` towards `params`: its pitch at once, the rest smoothly
    /// (R17).
    ///
    /// # Panics
    ///
    /// If `voice` is out of range.
    pub fn set_params(&mut self, voice: usize, params: &ModalParams) {
        let hz = sane(params.hz, 1.0, 1e6, 220.0);
        self.controls[voice].target = controls_of(params);
        if hz != self.hz[voice] {
            self.hz[voice] = hz;
            self.dirty[voice] = ALL;
            self.refresh(voice);
        }
    }

    /// Sets `voice` to `params` with nothing moving and nothing ringing: the
    /// state a new note starts from.
    ///
    /// # Panics
    ///
    /// If `voice` is out of range.
    pub fn reset_voice(&mut self, voice: usize, params: &ModalParams) {
        self.hz[voice] = sane(params.hz, 1.0, 1e6, 220.0);
        self.controls[voice] = Controls::new(controls_of(params));
        for row in 0..MAX_PARTIALS {
            self.x[row * self.stride + voice] = 0.0;
            self.y[row * self.stride + voice] = 0.0;
        }
        self.dirty[voice] = ALL;
        self.refresh(voice);
    }

    /// Whether every voice has fallen silent: no state is left to ring.
    #[must_use]
    pub fn is_silent(&self) -> bool {
        let used = self.active * self.stride;
        self.x[..used]
            .iter()
            .chain(&self.y[..used])
            .all(|&v| v == 0.0)
    }

    /// Renders `input.len() / stride` frames: `input` is one excitation value
    /// per voice per frame; `output` takes the left channel's voices, then
    /// the right's, for each frame. A resonator's output depends only on its
    /// excitation and its settings, never on how blocks fall (R16).
    ///
    /// # Panics
    ///
    /// If `input` isn't a whole number of frames or `output` isn't twice as
    /// long.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        simd::dispatch(self.run(input, output));
    }

    /// As [`process`](Self::process), with lanes at `level`, for tests and
    /// benchmarks that compare paths. Gives `false`, leaving `output` alone,
    /// if this CPU can't run `level`.
    ///
    /// # Panics
    ///
    /// As [`process`](Self::process).
    pub fn process_at(&mut self, level: Level, input: &[f32], output: &mut [f32]) -> bool {
        simd::dispatch_at(level, self.run(input, output)).is_ok()
    }

    fn run<'a>(&'a mut self, input: &'a [f32], output: &'a mut [f32]) -> Run<'a> {
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
        }
    }

    /// The control update: smoothing, refreshing what moved, flushing tiny
    /// state (R14, R17).
    fn tick(&mut self) {
        let alpha = smoothing(self.rate);
        if self.table_changed {
            self.table_changed = false;
            self.dirty.fill(ALL);
        }
        for voice in 0..self.voices {
            let moved = self.controls[voice].step(alpha);
            if moved & 0b11 != 0 {
                self.dirty[voice] |= DECAY;
            }
            if moved & 0b11_1100 != 0 {
                self.dirty[voice] |= GAIN;
            }
            if self.dirty[voice] != 0 {
                self.refresh(voice);
            }
        }
        let used = self.active * self.stride;
        for value in self.x[..used].iter_mut().chain(&mut self.y[..used]) {
            flush(value);
        }
    }

    /// Works out what `voice`'s dirty flags say changed, for the partials
    /// below the Nyquist cut-off.
    fn refresh(&mut self, voice: usize) {
        let dirty = std::mem::take(&mut self.dirty[voice]);
        let stride = self.stride;
        let at = move |row: usize| row * stride + voice;
        let hz = self.hz[voice];
        let c = self.controls[voice].current;
        if dirty & TUNING != 0 {
            let count = self.table.below(NYQUIST * self.rate / hz);
            self.count[voice] = count;
            self.active = self.count.iter().copied().max().unwrap_or(0);
            for row in 0..MAX_PARTIALS {
                let eps = if row < count {
                    2.0 * (std::f64::consts::PI * hz * self.table.ratio(row) / self.rate).sin()
                } else {
                    self.x[at(row)] = 0.0;
                    self.y[at(row)] = 0.0;
                    0.0
                };
                self.eps[at(row)] = eps as f32;
            }
        }
        let count = self.count[voice];
        if dirty & (TUNING | DECAY) != 0 {
            let (decay, material) = (c[0].exp().clamp(1e-3, 60.0), c[1].clamp(0.0, 1.0));
            for row in 0..MAX_PARTIALS {
                let (pole, inject) = if row < count {
                    // Partial n falls 60 dB in decay × r^(−2(1 − m)) (R5).
                    let seconds =
                        decay * (-2.0 * (1.0 - material) * self.table.ln_ratio(row)).exp();
                    let delta = -(-LN_1000 / (seconds.max(1.0 / self.rate) * self.rate)).exp_m1();
                    // The nearest `f32` below 1 at the slowest: a pole on the circle never decays.
                    let pole = ((1.0 - delta) as f32).min(1.0 - f32::EPSILON / 2.0);
                    (pole, (RESONANCE_GAIN * delta).min(1.0) as f32)
                } else {
                    (0.0, 0.0)
                };
                self.pole[at(row)] = pole;
                self.inject[at(row)] = inject;
            }
        }
        if dirty & (TUNING | GAIN) != 0 {
            let brightness = c[2].clamp(-1.0, 1.0);
            let mut total = [0.0_f64; 2];
            for row in 0..MAX_PARTIALS {
                let mut sides = [0.0_f64; 2];
                if row < count {
                    // The sine's amplitude for an impulse is 1 / cos(ω / 2).
                    let eps = f64::from(self.eps[at(row)]);
                    let base = (1.0 - eps * eps / 4.0).sqrt()
                        * (0.5 * brightness * self.table.ln_ratio(row)).exp()
                        * self.table.shape(row, c[3]);
                    for (side, position) in [c[4], c[5]].into_iter().enumerate() {
                        sides[side] = base * self.table.shape(row, position);
                        total[side] += sides[side].abs();
                    }
                }
                self.gain_l[at(row)] = sides[0] as f32;
                self.gain_r[at(row)] = sides[1] as f32;
            }
            // The gains sum to at most 1, so no setting grows the output (R13).
            let (left, right) = (
                (1.0 / total[0].max(1.0)) as f32,
                (1.0 / total[1].max(1.0)) as f32,
            );
            for row in 0..MAX_PARTIALS {
                self.gain_l[at(row)] *= left;
                self.gain_r[at(row)] *= right;
            }
        }
    }
}

/// A voice's controls as the smoother holds them.
fn controls_of(params: &ModalParams) -> [f64; 6] {
    [
        sane(params.decay, 1e-3, 60.0, 1.0).ln(),
        sane(params.material, 0.0, 1.0, 0.5),
        sane(params.brightness, -1.0, 1.0, 0.0),
        sane(params.hit, 0.0, 1.0, 0.0),
        sane(params.pos_l, 0.0, 1.0, 0.0),
        sane(params.pos_r, 0.0, 1.0, 0.0),
    ]
}

/// [`ModalBank::process`]'s kernel.
struct Run<'a> {
    bank: &'a mut ModalBank,
    input: &'a [f32],
    output: &'a mut [f32],
    frames: usize,
}

impl Kernel for Run<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: F32s>(self) {
        let Self {
            bank,
            input,
            output,
            frames,
        } = self;
        let stride = bank.stride;
        let groups = bank.voices.div_ceil(S::LANES);
        let mut done = 0;
        while done < frames {
            let chunk = (frames - done).min(CONTROL - bank.phase);
            for group in 0..groups {
                let lane = group * S::LANES;
                for frame in done..done + chunk {
                    let excitation = S::load(&input[frame * stride + lane..]);
                    let (mut left, mut right) = (S::splat(0.0), S::splat(0.0));
                    for row in 0..bank.active {
                        let at = row * stride + lane;
                        let eps = S::load(&bank.eps[at..]);
                        let pole = S::load(&bank.pole[at..]);
                        let y =
                            S::load(&bank.inject[at..]).mul_add(excitation, S::load(&bank.y[at..]));
                        let x = eps.mul_add(y, S::load(&bank.x[at..]));
                        let y = (-eps).mul_add(x, y);
                        let (x, y) = (x * pole, y * pole);
                        x.store(&mut bank.x[at..]);
                        y.store(&mut bank.y[at..]);
                        left = S::load(&bank.gain_l[at..]).mul_add(x, left);
                        right = S::load(&bank.gain_r[at..]).mul_add(x, right);
                    }
                    left.store(&mut output[frame * 2 * stride + lane..]);
                    right.store(&mut output[(frame * 2 + 1) * stride + lane..]);
                }
            }
            bank.phase += chunk;
            done += chunk;
            if bank.phase == CONTROL {
                bank.phase = 0;
                bank.tick();
            }
        }
    }
}
