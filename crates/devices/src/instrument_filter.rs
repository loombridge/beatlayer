//! The instruments' filter (the instrument filter spec, R1 to R17): five
//! types at 12 or 24 dB per octave, in the circuits Clean, OSR, MS2, SMP and
//! PRD, with Drive and Morph, run per voice and per frame.
//!
//! Clean is the plug-in kit's state-variable filter, so Simpler and Sampler
//! sets saved before this filter render as they did (R3, R15). The other
//! circuits are Beatlayer's own zero-delay-feedback structures, one voice
//! and channel at a time: OSR a state-variable section whose state is held
//! by a hard clip, MS2 and SMP a Sallen-Key section whose feedback softens,
//! PRD a transistor ladder. A change of type, slope or circuit crossfades to
//! the new structure over 5 ms (R11).

use std::f64::consts::{FRAC_1_SQRT_2, PI};

use lspk_dsp::filter::{FilterBank, FilterType, VoiceParams, hertz_of};

/// The circuits, in the chooser's order.
pub const CIRCUITS: [&str; 5] = ["Clean", "OSR", "MS2", "SMP", "PRD"];

/// The filter's Type, in the Sampler chooser's order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Morph,
}

impl Kind {
    /// The type the chooser's `index` names.
    #[must_use]
    pub fn of(index: usize) -> Self {
        [
            Self::LowPass,
            Self::HighPass,
            Self::BandPass,
            Self::Notch,
            Self::Morph,
        ][index.min(4)]
    }
}

/// A circuit, by its place in [`CIRCUITS`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Circuit {
    Clean,
    Osr,
    Ms2,
    Smp,
    Prd,
}

impl Circuit {
    /// The circuit the chooser's `index` names.
    #[must_use]
    pub fn of(index: usize) -> Self {
        [Self::Clean, Self::Osr, Self::Ms2, Self::Smp, Self::Prd][index.min(4)]
    }
}

/// The circuits a type plays (R2): Clean and OSR on every type, the other
/// three on low-pass and high-pass.
#[must_use]
pub fn offered(kind: Kind) -> &'static [Circuit] {
    match kind {
        Kind::LowPass | Kind::HighPass => &[
            Circuit::Clean,
            Circuit::Osr,
            Circuit::Ms2,
            Circuit::Smp,
            Circuit::Prd,
        ],
        _ => &[Circuit::Clean, Circuit::Osr],
    }
}

/// What the filter is: its type, slope and the circuit that plays.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Setup {
    pub kind: Kind,
    pub slope24: bool,
    pub circuit: Circuit,
}

impl Setup {
    /// The setup the choosers' indices give. A circuit the type doesn't
    /// offer plays as Clean (R2); its parameter keeps its value.
    #[must_use]
    pub fn new(kind: usize, slope24: bool, circuit: usize) -> Self {
        let kind = Kind::of(kind);
        let circuit = Circuit::of(circuit);
        Self {
            kind,
            slope24,
            circuit: if offered(kind).contains(&circuit) {
                circuit
            } else {
                Circuit::Clean
            },
        }
    }

    /// Whether Drive acts: on the circuits but Clean, for low-pass,
    /// high-pass and band-pass (R9).
    fn drives(self) -> bool {
        self.circuit != Circuit::Clean
            && matches!(self.kind, Kind::LowPass | Kind::HighPass | Kind::BandPass)
    }
}

/// A control's values for a block: one per frame, shared by every voice, or
/// one per frame and voice (laid out like the cutoff).
#[derive(Clone, Copy)]
pub enum Per<'a> {
    Shared(&'a [f32]),
    Voice(&'a [f32]),
}

impl Per<'_> {
    #[inline]
    fn at(self, frame: usize, stride: usize, lane: usize) -> f32 {
        match self {
            Self::Shared(values) => values[frame],
            Self::Voice(values) => values[frame * stride + lane],
        }
    }
}

/// What a block's filtering reads, for every frame (R11).
#[derive(Clone, Copy)]
pub struct PerFrame<'a> {
    /// Each voice's cutoff in the kit's semitones (69 is A4 at 440 Hz): per
    /// frame, one value per slot.
    pub cutoff: &'a [f32],
    /// Resonance, Morph and Drive, each 0 to 1 (Drive of its 24 dB).
    pub res: Per<'a>,
    pub morph: Per<'a>,
    pub drive: Per<'a>,
}

/// The kit's reference pitch.
const REFERENCE_HZ: f32 = 440.0;
/// The highest cutoff, as a share of the sample rate (R12).
const CUTOFF_LIMIT: f64 = 0.45;
/// How long a change of structure crossfades, in seconds (R11).
const FADE_SECONDS: f64 = 0.005;
/// Drive's range, in decibels (R9).
const DRIVE_DB: f64 = 24.0;
/// The most Q a section has at full Resonance.
const Q_MAX: f64 = 10.0;
/// The two sections of a 24 dB response: Butterworth's Q at no Resonance.
const Q_FIRST: f64 = 0.541_196_1;
const Q_SECOND: f64 = 1.306_563;
/// OSR's state, held by a hard clip (R5).
const OSR_STATE: f64 = 1.0;
/// OSR's, MS2's and SMP's output bound, just under +6 dBFS (R5, R6).
const BOUND: f64 = 1.9;
/// MS2's and SMP's feedback softens above this (R6, R8): SMP's later.
const MS2_SOFT: f64 = 2.0;
const SMP_SOFT: f64 = 6.0;
/// PRD's input stage saturates at this level, and SMP's later (R7, R8).
const PRD_INPUT_SOFT: f64 = 1.0;
const SMP_INPUT_SOFT: f64 = 1.4;
/// PRD's ladder: its feedback at full Resonance (it oscillates from 4), the
/// level its stages soften at, the share of its feedback its output makes
/// up for, and its output bound, just under +12 dBFS (R7).
const LADDER_K: f64 = 4.2;
const LADDER_SOFT: f64 = 6.0;
const LADDER_COMPENSATION: f64 = 0.5;
const LADDER_BOUND: f64 = 3.9;
/// A value this small is left as zero rather than a denormal (R12).
const FLOOR: f64 = 1e-20;
/// The most state variables a voice's channel holds: two Sallen-Key
/// sections of three.
const STATES: usize = 6;

/// The kit's four types a bank set runs for `kind` and `slope24`: the one
/// the type names in every bank, or for Morph the four it sweeps through,
/// low-pass, band-pass, high-pass and notch.
fn kit_types(kind: Kind, slope24: bool) -> [FilterType; 4] {
    let at = |kind: Kind| match (kind, slope24) {
        (Kind::LowPass, false) => FilterType::Lp12,
        (Kind::LowPass, true) => FilterType::Lp24,
        (Kind::HighPass, false) => FilterType::Hp12,
        (Kind::HighPass, true) => FilterType::Hp24,
        (Kind::BandPass, false) => FilterType::Bp12,
        (Kind::BandPass, true) => FilterType::Bp24,
        (_, false) => FilterType::Notch12,
        (_, true) => FilterType::Notch24,
    };
    if kind == Kind::Morph {
        [
            at(Kind::LowPass),
            at(Kind::BandPass),
            at(Kind::HighPass),
            at(Kind::Notch),
        ]
    } else {
        [at(kind); 4]
    }
}

/// The filter of every voice slot of a device.
pub struct InstrumentFilter {
    /// The structure playing and the one it replaced, fading out.
    stages: [Stage; 2],
    current: usize,
    setup: Setup,
    fade_left: usize,
    fade_frames: usize,
    /// Whether a block has played, before which a new setup replaces the
    /// first with nothing to fade from.
    played: bool,
    /// The slots that started afresh since the last block, and the frame
    /// each started on.
    fresh: Vec<(u16, u32)>,
    stride: usize,
    /// Work space for a block: the outgoing structure's audio, and Morph's
    /// two buffers.
    scratch: [Vec<f32>; 3],
}

impl InstrumentFilter {
    /// A filter for `slots` voices of two channels at `rate`, for blocks of
    /// up to `max_frames`. Allocates, so make it off the audio thread.
    #[must_use]
    pub fn new(rate: f32, slots: usize, max_frames: usize) -> Self {
        let setup = Setup::new(0, false, 0);
        let mut stages = [
            Stage::new(rate, slots, setup),
            Stage::new(rate, slots, setup),
        ];
        for stage in &mut stages {
            stage.start(setup);
        }
        let stride = stages[0].banks[0].stride();
        Self {
            stages,
            current: 0,
            setup,
            fade_left: 0,
            fade_frames: ((f64::from(rate) * FADE_SECONDS) as usize).max(1),
            played: false,
            fresh: Vec::with_capacity(slots),
            stride,
            scratch: core::array::from_fn(|_| vec![0.0; max_frames.max(1) * 2 * stride]),
        }
    }

    /// Has a new voice start from the settings its slot last held and glide
    /// to the current ones, as Simpler's voices did before this filter, so
    /// older sets render as they did (R15).
    #[must_use]
    pub fn starting_from_last_settings(mut self) -> Self {
        for stage in &mut self.stages {
            stage.from_last = true;
        }
        self
    }

    /// How many values a frame of the per-voice buffers holds.
    #[must_use]
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// Moves to `setup`: the structure that played fades out as the new one
    /// starts from silence (R11). Nothing happens if it is the one playing.
    pub fn set_setup(&mut self, setup: Setup) {
        if setup == self.setup {
            return;
        }
        self.setup = setup;
        // shortcut: a change during a fade reuses the fading structure for the
        // new one, cutting that fade short; upgrade to a third structure if
        // changes under 5 ms apart ever click.
        if self.played {
            self.current = 1 - self.current;
            self.fade_left = self.fade_frames;
        }
        self.stages[self.current].start(setup);
    }

    /// Starts slot `slot`'s voice afresh: its state clears and its settings
    /// take their values at `frame`, where its sound starts, without
    /// gliding.
    pub fn reset_voice(&mut self, slot: usize, frame: usize) {
        if !self
            .fresh
            .iter()
            .any(|(lane, _)| usize::from(*lane) == slot)
        {
            self.fresh.push((slot as u16, frame as u32));
        }
    }

    /// Filters `frames` frames of `audio` (per frame, then per channel, one
    /// value per slot) for the first `hi` slots, in place.
    pub fn process(&mut self, hi: usize, frames: usize, c: &PerFrame<'_>, audio: &mut [f32]) {
        self.played = true;
        let used = frames * 2 * self.stride;
        let [work, a, b] = &mut self.scratch;
        let fading = self.fade_left > 0;
        if fading {
            work[..used].copy_from_slice(&audio[..used]);
            let old = &mut self.stages[1 - self.current];
            old.run(
                hi,
                frames,
                c,
                &self.fresh,
                &mut work[..used],
                [&mut *a, &mut *b],
            );
        }
        self.stages[self.current].run(hi, frames, c, &self.fresh, &mut audio[..used], [a, b]);
        if fading {
            let stride = self.stride;
            for frame in 0..frames {
                let left = self.fade_left.saturating_sub(frame);
                let old = left as f32 / self.fade_frames as f32;
                let at = frame * 2 * stride;
                for i in at..at + 2 * stride {
                    audio[i] += (work[i] - audio[i]) * old;
                }
            }
            self.fade_left = self.fade_left.saturating_sub(frames);
        }
        self.fresh.clear();
    }
}

/// One structure's filter: the kit's banks for Clean, the scalar states for
/// the circuits.
struct Stage {
    setup: Setup,
    banks: Vec<FilterBank>,
    /// The settings last handed to each slot's bank lanes.
    last: Vec<VoiceParams>,
    /// Each slot's two channels' states.
    state: Vec<[f64; STATES]>,
    /// Whether a voice starts from the settings its slot last held.
    from_last: bool,
    /// The frame each slot's voice started on in this block, or 0.
    first: Vec<u32>,
    rate: f64,
}

impl Stage {
    fn new(rate: f32, slots: usize, setup: Setup) -> Self {
        Self {
            setup,
            banks: (0..4).map(|_| FilterBank::new(rate, slots, 2)).collect(),
            last: vec![VoiceParams::default(); slots],
            state: vec![[0.0; STATES]; slots * 2],
            first: vec![0; slots],
            from_last: false,
            rate: f64::from(rate),
        }
    }

    /// Starts afresh as `setup`.
    fn start(&mut self, setup: Setup) {
        self.setup = setup;
        self.state.fill([0.0; STATES]);
        let types = kit_types(setup.kind, setup.slope24);
        self.last.fill(VoiceParams::default());
        for (bank, kind) in self.banks.iter_mut().zip(types) {
            bank.set_type(kind);
            for lane in 0..self.last.len() {
                bank.set_params(lane, &VoiceParams::default());
            }
            bank.reset();
        }
    }

    fn run(
        &mut self,
        hi: usize,
        frames: usize,
        c: &PerFrame<'_>,
        fresh: &[(u16, u32)],
        audio: &mut [f32],
        scratch: [&mut Vec<f32>; 2],
    ) {
        self.first.fill(0);
        for &(lane, frame) in fresh {
            self.first[usize::from(lane)] = frame;
        }
        if self.setup.circuit == Circuit::Clean {
            self.clean(hi, frames, c, fresh, audio, scratch);
        } else {
            self.circuit(hi, frames, c, fresh, audio);
        }
    }

    /// Clean: the kit's banks, in stretches over which no voice's
    /// resonance moves. Morph runs four banks and crossfades the two
    /// neighbouring its place.
    fn clean(
        &mut self,
        hi: usize,
        frames: usize,
        c: &PerFrame<'_>,
        fresh: &[(u16, u32)],
        audio: &mut [f32],
        [work, sum]: [&mut Vec<f32>; 2],
    ) {
        let stride = self.banks[0].stride();
        let morph = self.setup.kind == Kind::Morph;
        let banks = if morph { 4 } else { 1 };
        // A voice's values before its sound starts are what an earlier
        // voice left in the buffers: they count from its first frame.
        let first = &self.first;
        let res = |lane: usize, frame: usize| {
            c.res
                .at(frame.max(first[lane] as usize), stride, lane)
                .clamp(0.0, 1.0)
        };
        let mut start = 0;
        while start < frames {
            for lane in 0..hi {
                let params = VoiceParams {
                    resonance: res(lane, start),
                    ..VoiceParams::default()
                };
                let is_fresh = start == 0 && fresh.iter().any(|(l, _)| usize::from(*l) == lane);
                let from = if is_fresh && self.from_last {
                    self.last[lane]
                } else {
                    params
                };
                if is_fresh || params != self.last[lane] {
                    for bank in &mut self.banks[..banks] {
                        if is_fresh {
                            bank.reset_voice(lane, &from);
                        }
                        if from != params || !is_fresh {
                            bank.set_params(lane, &params);
                        }
                    }
                    self.last[lane] = params;
                }
            }
            let mut end = start + 1;
            while end < frames && (0..hi).all(|lane| res(lane, end) == res(lane, start)) {
                end += 1;
            }
            let cutoff = &c.cutoff[start * stride..end * stride];
            let span = start * 2 * stride..end * 2 * stride;
            if morph {
                let input = &audio[span.clone()];
                let sum = &mut sum[..span.len()];
                sum.fill(0.0);
                for (n, bank) in self.banks.iter_mut().enumerate() {
                    let work = &mut work[..span.len()];
                    work.copy_from_slice(input);
                    bank.process_voices(hi, cutoff, work);
                    for frame in start..end {
                        let at = (frame - start) * 2 * stride;
                        for lane in 0..hi {
                            // The Morph's place round the four stops: the
                            // two neighbours share the unit between them.
                            let place = 4.0 * c.morph.at(frame, stride, lane).clamp(0.0, 1.0);
                            let first = (place as usize).min(3);
                            let share = place - first as f32;
                            let w = if n == first {
                                1.0 - share
                            } else if n == (first + 1) % 4 {
                                share
                            } else {
                                continue;
                            };
                            sum[at + lane] += w * work[at + lane];
                            sum[at + stride + lane] += w * work[at + stride + lane];
                        }
                    }
                }
                audio[span.clone()].copy_from_slice(sum);
            } else {
                self.banks[0].process_voices(hi, cutoff, &mut audio[span]);
            }
            start = end;
        }
    }

    /// OSR, MS2, SMP and PRD: each slot's channels through the scalar
    /// section, a voice at a time.
    fn circuit(
        &mut self,
        hi: usize,
        frames: usize,
        c: &PerFrame<'_>,
        fresh: &[(u16, u32)],
        audio: &mut [f32],
    ) {
        let stride = self.banks[0].stride();
        let setup = self.setup;
        for lane in 0..hi {
            let states = &mut self.state[lane * 2..lane * 2 + 2];
            if fresh.iter().any(|(l, _)| usize::from(*l) == lane) {
                states.fill([0.0; STATES]);
            }
            // A voice that heard nothing and holds nothing makes nothing.
            let silent = (0..frames).all(|frame| {
                let at = frame * 2 * stride + lane;
                audio[at] == 0.0 && audio[at + stride] == 0.0
            });
            if silent && states.iter().all(|s| s.iter().all(|v| *v == 0.0)) {
                continue;
            }
            let mut coef = Coef::of(setup, 0.0, 0.0, 0.0, 0.0);
            let mut seen = [f32::NAN; 4];
            for frame in 0..frames {
                let now = [
                    c.cutoff[frame * stride + lane],
                    c.res.at(frame, stride, lane),
                    c.morph.at(frame, stride, lane),
                    c.drive.at(frame, stride, lane),
                ];
                // The cutoff moves most often, and costs a `tan`; the rest costs
                // a few `powf`s.
                if now[0] != seen[0] {
                    coef.g = prewarp(self.rate, now[0]);
                }
                if now[1..] != seen[1..] {
                    coef = Coef::of(setup, coef.g, now[1], now[2], now[3]);
                }
                seen = now;
                for (channel, state) in states.iter_mut().enumerate() {
                    let at = (frame * 2 + channel) * stride + lane;
                    let y = tick(setup, &coef, state, f64::from(audio[at]));
                    audio[at] = y as f32;
                }
            }
        }
    }
}

/// What a frame's filtering needs, worked out from its cutoff and controls.
struct Coef {
    /// The prewarped cutoff, `tan(pi fc / rate)`.
    g: f64,
    /// Each section's Q.
    q: [f64; 2],
    /// Drive's gain into the circuit.
    gain: f64,
    /// Morph's place round the four stops, 0 to 4.
    place: f64,
    /// The ladder's feedback.
    k: f64,
}

/// The prewarped cutoff of `pitch` (the kit's semitones) at `rate`, held to
/// 0.45 of it (R12).
fn prewarp(rate: f64, pitch: f32) -> f64 {
    let hz = f64::from(hertz_of(pitch, REFERENCE_HZ));
    let hz = if hz.is_finite() { hz } else { 1_000.0 };
    (PI * hz.clamp(1.0, rate * CUTOFF_LIMIT) / rate).tan()
}

impl Coef {
    /// The coefficients for prewarped cutoff `g` and the controls.
    fn of(setup: Setup, g: f64, res: f32, morph: f32, drive: f32) -> Self {
        let r = if res.is_finite() {
            f64::from(res).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q_of = |q0: f64| q0 * (Q_MAX / q0).powf(r);
        let q = if setup.slope24 {
            [Q_FIRST, q_of(Q_SECOND)]
        } else {
            [q_of(FRAC_1_SQRT_2), 0.0]
        };
        let d = if drive.is_finite() {
            f64::from(drive).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let m = if morph.is_finite() {
            f64::from(morph).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            g,
            q,
            gain: if setup.drives() {
                10f64.powf(d * DRIVE_DB / 20.0)
            } else {
                1.0
            },
            place: 4.0 * m,
            k: LADDER_K * r,
        }
    }
}

fn soft(x: f64, a: f64) -> f64 {
    a * (x / a).tanh()
}

/// `x` held below `bound`: left alone up to half of it, then rounded off.
fn limit(x: f64, bound: f64) -> f64 {
    let knee = bound / 2.0;
    if x.abs() <= knee {
        x
    } else {
        x.signum() * (knee + soft(x.abs() - knee, knee))
    }
}

fn flush(x: f64) -> f64 {
    if x.abs() < FLOOR { 0.0 } else { x }
}

/// One sample through the circuit, left in `state`. A result that isn't a
/// number clears the state, so one bad value can't stay (R12).
fn tick(setup: Setup, c: &Coef, state: &mut [f64; STATES], x: f64) -> f64 {
    let x = x * c.gain;
    let y = match setup.circuit {
        Circuit::Osr => osr(setup, c, state, x),
        Circuit::Ms2 => sallen_key(setup, c, state, x, MS2_SOFT),
        Circuit::Smp => sallen_key(setup, c, state, soft(x, SMP_INPUT_SOFT), SMP_SOFT),
        _ => ladder(setup, c, state, soft(x, PRD_INPUT_SOFT)),
    };
    if y.is_finite() {
        y
    } else {
        *state = [0.0; STATES];
        0.0
    }
}

/// The section's taps, by Morph's place and the type.
fn taps(setup: Setup, place: f64, [lp, bp, hp, notch]: [f64; 4]) -> f64 {
    match setup.kind {
        Kind::LowPass => lp,
        Kind::HighPass => hp,
        Kind::BandPass => bp,
        Kind::Notch => notch,
        Kind::Morph => {
            let all = [lp, bp, hp, notch];
            let first = (place as usize).min(3);
            let share = place - first as f64;
            all[first] * (1.0 - share) + all[(first + 1) % 4] * share
        }
    }
}

fn sections(setup: Setup) -> usize {
    if setup.slope24 { 2 } else { 1 }
}

/// OSR (R5): state-variable sections whose band-pass state is clipped hard.
fn osr(setup: Setup, c: &Coef, state: &mut [f64; STATES], x: f64) -> f64 {
    let g = c.g;
    let mut y = x;
    for s in 0..sections(setup) {
        let k = 1.0 / c.q[s];
        let a1 = 1.0 / (1.0 + g * (g + k));
        let (a2, a3) = (g * a1, g * g * a1);
        let [ic1, ic2] = [state[2 * s], state[2 * s + 1]];
        let v3 = y - ic2;
        let v1 = a1 * ic1 + a2 * v3;
        let v2 = ic2 + a2 * ic1 + a3 * v3;
        state[2 * s] = flush(2.0 * v1 - ic1).clamp(-OSR_STATE, OSR_STATE);
        state[2 * s + 1] = flush(2.0 * v2 - ic2);
        let hp = y - k * v1 - v2;
        y = taps(setup, c.place, [v2, k * v1, hp, v2 + hp]);
    }
    y.clamp(-BOUND, BOUND)
}

/// MS2 and SMP (R6, R8): Sallen-Key sections. A section's feedback gain
/// `K = 3 - 1/Q` is scaled by how far the output has softened at `soft`, so
/// the louder the section the less it resonates.
fn sallen_key(setup: Setup, c: &Coef, state: &mut [f64; STATES], x: f64, soft_at: f64) -> f64 {
    let g = c.g;
    let mut y = x;
    for s in 0..sections(setup) {
        let st = &mut state[3 * s..3 * s + 3];
        let k_full = 3.0 - 1.0 / c.q[s];
        let out = k_full * st[2];
        let kappa = if out.abs() < 1e-9 {
            1.0
        } else {
            soft(out, soft_at) / out
        };
        let k = k_full * kappa;
        let det = 1.0 + (3.0 - k) * g + g * g;
        let drive = st[0] + g * y;
        let u = ((1.0 - g * (k - 1.0)) * drive - g * (2.0 * k - 1.0) * st[1]) / det;
        let vb = (g * drive + (1.0 + 2.0 * g) * st[1]) / det;
        st[0] = flush(2.0 * u - st[0]);
        st[1] = flush(2.0 * vb - st[1]);
        st[2] = flush(vb);
        let band = u + (k - 1.0) * vb;
        y = if setup.kind == Kind::HighPass {
            y - vb - (3.0 - k) * band
        } else {
            vb
        };
    }
    limit(y, BOUND)
}

/// PRD (R7): a four-pole ladder with the type's stage as output, whose
/// stages soften but whose feedback has no limit.
fn ladder(setup: Setup, c: &Coef, state: &mut [f64; STATES], x: f64) -> f64 {
    let high = setup.kind == Kind::HighPass;
    let (g, k) = (c.g, c.k);
    let (big, small) = (g / (1.0 + g), 1.0 / (1.0 + g));
    // The part of the last stage's output that doesn't depend on the input
    // now, and how much of the input now it holds.
    let (sigma, through) = if high {
        (
            -(small.powi(4) * state[0]
                + small.powi(3) * state[1]
                + small.powi(2) * state[2]
                + small * state[3]),
            small.powi(4),
        )
    } else {
        (
            big.powi(3) * small * state[0]
                + big.powi(2) * small * state[1]
                + big * small * state[2]
                + small * state[3],
            big.powi(4),
        )
    };
    let mut u = (x - k * sigma) / (1.0 + k * through);
    let mut out = 0.0;
    for s in 0..4 {
        let low = big * u + small * state[s];
        let stage = if high { u - low } else { low };
        state[s] = flush(soft(2.0 * low - state[s], LADDER_SOFT));
        u = stage;
        if s == if setup.slope24 { 3 } else { 1 } {
            out = stage;
        }
    }
    limit(out * (1.0 + LADDER_COMPENSATION * k), LADDER_BOUND)
}
