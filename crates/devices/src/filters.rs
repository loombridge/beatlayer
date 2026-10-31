//! The Auto Filter's filter types and circuits (WP-5.20; the auto-filter
//! spec R2 to R5), shared with the devices that filter the same way, such as
//! Roar (the roar spec R6): a state-variable section's design and states, the
//! types built on them, and the curves they are held to.

use core::f64::consts::{FRAC_1_SQRT_2, PI, TAU};

/// The cutoff range, in Hz.
pub(crate) const MIN_HZ: f64 = 20.0;
pub(crate) const MAX_HZ: f64 = 20_000.0;

/// Res 100 % is this Q; 0 % is the Butterworth Q, a flat top.
pub(crate) const Q_MAX: f64 = 10.0;

/// Notch + LP's notch has this Q, and sits this many octaves above the
/// cutoff at Morph 0 %.
pub(crate) const NOTCH_Q: f64 = 2.0;
pub(crate) const NOTCH_OCTAVES: f64 = 3.0;

/// The most feedback the comb takes, at Res 100 %.
pub(crate) const COMB_MAX: f64 = 0.95;

/// Vowel's formants Q at Res 0 %, and how much more Res 100 % adds.
pub(crate) const FORMANT_Q: f64 = 8.0;



/// Where the circuits soften the sections' state, in the state's own units:
/// the state is the signal times about Q, so a resonant section at a high
/// level reaches them while a −40 dBFS signal stays well inside (the
/// softening takes a share of the state on every sample).
pub(crate) const SOFT_LOOSE: f64 = 8.0;
pub(crate) const SOFT_TIGHT: f64 = 3.0;


/// A Vowel's formants: for a, e, i, o and u, each formant's frequency in Hz
/// and its relative level. Beatlayer's own, from published averages of
/// spoken vowels.
pub(crate) const FORMANTS: [[(f64, f64); 3]; 5] = [
    [(730.0, 1.0), (1090.0, 0.45), (2440.0, 0.2)],
    [(530.0, 1.0), (1840.0, 0.45), (2480.0, 0.2)],
    [(270.0, 1.0), (2290.0, 0.45), (3010.0, 0.2)],
    [(570.0, 1.0), (840.0, 0.45), (2410.0, 0.2)],
    [(300.0, 1.0), (870.0, 0.45), (2240.0, 0.2)],
];

/// A filter type, by its place in [`TYPES`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    NotchLowPass,
    Morph,
    Dj,
    Comb,
    Resampling,
    Vowel,
}

impl Kind {
    pub(crate) fn of(index: usize) -> Self {
        const KINDS: [Kind; 10] = [
            Kind::LowPass,
            Kind::HighPass,
            Kind::BandPass,
            Kind::Notch,
            Kind::NotchLowPass,
            Kind::Morph,
            Kind::Dj,
            Kind::Comb,
            Kind::Resampling,
            Kind::Vowel,
        ];
        KINDS[index.min(KINDS.len() - 1)]
    }
}

/// A circuit, by its place in [`CIRCUITS`]: where the sections' state is
/// softened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Circuit {
    /// Nowhere: clean, distorting only through Drive.
    Svf,
    /// In both integrators, so the distortion is fed back.
    Dfm,
    /// In the band-pass integrator, tightly, so the resonance is bounded.
    Ms2,
    /// In the low-pass integrator only, so the resonance isn't limited.
    Prd,
}

impl Circuit {
    pub(crate) fn of(index: usize) -> Self {
        [Self::Svf, Self::Dfm, Self::Ms2, Self::Prd][index.min(3)]
    }
}

/// A complex number, for the curves.
#[derive(Clone, Copy)]
pub(crate) struct C(f64, f64);

impl C {
    pub(crate) fn mul(self, o: C) -> C {
        C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    pub(crate) fn div(self, o: C) -> C {
        let d = o.0 * o.0 + o.1 * o.1;
        C(
            (self.0 * o.0 + self.1 * o.1) / d,
            (self.1 * o.0 - self.0 * o.1) / d,
        )
    }
    pub(crate) fn add(self, o: C) -> C {
        C(self.0 + o.0, self.1 + o.1)
    }
    pub(crate) fn scale(self, k: f64) -> C {
        C(self.0 * k, self.1 * k)
    }
    pub(crate) fn abs(self) -> f64 {
        self.0.hypot(self.1)
    }
}

/// What a filter does at one frame: its type and settings. The cutoffs are
/// worked out once a frame; the section coefficients follow from them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Plan {
    pub(crate) kind: Kind,
    /// Sections in the cascade (0 for the types that have none).
    pub(crate) stages: usize,
    /// Morph's 6 dB slope: one pole.
    pub(crate) one_pole: bool,
    pub(crate) circuit: Circuit,
    /// Freq, and the factor the LFO and envelope move it by.
    pub(crate) hz: f64,
    pub(crate) shift: f64,
    /// The first section's Q.
    pub(crate) q: f64,
    /// Res, Morph and Formant from 0 to 1; Control from −1 to 1.
    pub(crate) res: f64,
    pub(crate) morph: f64,
    pub(crate) control: f64,
    pub(crate) pitch: f64,
    pub(crate) formant: f64,
}

/// The knobs a plan takes besides the choosers: Freq in Hz, Res, Morph and
/// Formant from 0 to 1, Control from −1 to 1, and Pitch in semitones.
#[derive(Clone, Copy)]
pub(crate) struct Knobs {
    pub(crate) hz: f64,
    pub(crate) res: f64,
    pub(crate) morph: f64,
    pub(crate) control: f64,
    pub(crate) pitch: f64,
    pub(crate) formant: f64,
}

impl Plan {
    pub(crate) fn new(kind: usize, slope: usize, circuit: usize, knobs: Knobs) -> Self {
        let Knobs {
            hz,
            res,
            morph,
            control,
            pitch,
            formant,
        } = knobs;
        let kind = Kind::of(kind);
        let (stages, one_pole) = match kind {
            Kind::Morph => match slope {
                0 => (1, true),
                1 => (1, false),
                2 => (2, false),
                _ => (4, false),
            },
            Kind::LowPass | Kind::HighPass | Kind::BandPass | Kind::Notch | Kind::NotchLowPass => {
                (if slope >= 2 { 2 } else { 1 }, false)
            }
            Kind::Dj => (1, false),
            Kind::Comb | Kind::Resampling | Kind::Vowel => (0, false),
        };
        // The circuits act on these types alone.
        let circuit = match kind {
            Kind::LowPass | Kind::HighPass | Kind::BandPass | Kind::Notch | Kind::Morph => {
                Circuit::of(circuit)
            }
            _ => Circuit::Svf,
        };
        Self {
            kind,
            stages,
            one_pole,
            circuit,
            hz,
            shift: 1.0,
            q: q_of(res),
            res,
            morph,
            control,
            pitch,
            formant,
        }
    }

    /// The cutoff the type plays at, after the modulation.
    pub(crate) fn cutoff(&self) -> f64 {
        self.hz * self.shift
    }

    /// DJ's cutoff and Q: above zero a high-pass sweeping up from 20 Hz,
    /// below zero a low-pass sweeping down from 20 kHz, the resonance
    /// growing with the distance from zero.
    pub(crate) fn dj(&self) -> (bool, f64, f64) {
        let c = self.control.abs();
        let q = FRAC_1_SQRT_2 + (self.q - FRAC_1_SQRT_2) * c;
        let hz = if self.control >= 0.0 {
            MIN_HZ * (MAX_HZ / MIN_HZ).powf(c)
        } else {
            MAX_HZ * (MIN_HZ / MAX_HZ).powf(c)
        };
        (self.control >= 0.0, hz * self.shift, q)
    }

    /// Vowel's three formants at the position and pitch: frequency, level
    /// and Q.
    pub(crate) fn formants(&self) -> [(f64, f64, f64); 3] {
        let at = (self.formant * 4.0).min(4.0);
        let from = (at.floor() as usize).min(3);
        let t = at - from as f64;
        let shift = self.shift * (self.pitch / 12.0).exp2();
        let q = FORMANT_Q * (1.0 + self.res);
        core::array::from_fn(|i| {
            let (a, b) = (FORMANTS[from][i], FORMANTS[from + 1][i]);
            let hz = a.0 * (b.0 / a.0).powf(t) * shift;
            let level = a.1 + (b.1 - a.1) * t;
            (hz, level + (1.0 - level) * self.morph, q)
        })
    }

    /// The comb's feedback, from Res.
    pub(crate) fn feedback(&self) -> f64 {
        COMB_MAX * self.res
    }

    /// Notch + LP's notch cutoff: the cutoff's own at Morph 100 %.
    pub(crate) fn notch_hz(&self) -> f64 {
        self.cutoff() * (NOTCH_OCTAVES * (1.0 - self.morph)).exp2()
    }

    /// The small-signal response at `hz` at sample rate `rate`: the
    /// spec's curve, which the filters play. Drive, the circuits'
    /// softening, Clip and the mix are not in it.
    pub(crate) fn response(&self, hz: f64, rate: f64) -> C {
        let w = (PI * hz / rate).tan();
        // A section's lp, normalised band-pass, high-pass and notch at its
        // cutoff and Q.
        let section = |fc: f64, q: f64| {
            let g = (PI * fc.clamp(1.0, rate * 0.49) / rate).tan();
            let (x, k) = (w / g, 1.0 / q);
            let den = C(1.0 - x * x, k * x);
            let one = C(1.0, 0.0);
            [
                one.div(den),
                C(0.0, k * x).div(den),
                C(-x * x, 0.0).div(den),
                C(1.0 - x * x, 0.0).div(den),
            ]
        };
        let q_of = |s: usize| if s == 0 { self.q } else { FRAC_1_SQRT_2 };
        let fc = self.cutoff();
        match self.kind {
            Kind::LowPass | Kind::HighPass | Kind::BandPass | Kind::Notch => {
                let pick = match self.kind {
                    Kind::LowPass => 0,
                    Kind::BandPass => 1,
                    Kind::HighPass => 2,
                    _ => 3,
                };
                (0..self.stages).fold(C(1.0, 0.0), |acc, s| acc.mul(section(fc, q_of(s))[pick]))
            }
            Kind::NotchLowPass => (0..self.stages)
                .fold(section(self.notch_hz(), NOTCH_Q)[3], |acc, s| {
                    acc.mul(section(fc, q_of(s))[0])
                }),
            Kind::Morph if self.one_pole => {
                let g = (PI * fc.clamp(1.0, rate * 0.49) / rate).tan();
                let x = w / g;
                let lp = C(1.0, 0.0).div(C(1.0, x));
                let hp = C(0.0, x).div(C(1.0, x));
                lp.scale(1.0 - self.morph).add(hp.scale(self.morph))
            }
            Kind::Morph => (0..self.stages).fold(C(1.0, 0.0), |acc, s| {
                let [lp, bp, hp, _] = section(fc, q_of(s));
                acc.mul(blend(self.morph, lp, bp, hp))
            }),
            Kind::Dj => {
                let (high, hz, q) = self.dj();
                section(hz, q)[if high { 2 } else { 0 }]
            }
            Kind::Comb => {
                let c = self.feedback();
                let delay = rate / fc.max(1.0);
                let z = C(
                    (TAU * hz * delay / rate).cos(),
                    -(TAU * hz * delay / rate).sin(),
                );
                let one = C(1.0, 0.0);
                let peaks = one.scale(1.0 - c).div(one.add(z.scale(-c)));
                let notches = one.add(z.scale(-c)).scale(1.0 / (1.0 + c));
                peaks.scale(1.0 - self.morph).add(notches.scale(self.morph))
            }
            Kind::Resampling => C(1.0, 0.0),
            Kind::Vowel => self
                .formants()
                .iter()
                .fold(C(0.0, 0.0), |acc, &(f, level, q)| {
                    acc.add(section(f, q)[1].scale(level))
                }),
        }
    }
}

/// Res from 0 to 1 as a Q, exponentially from Butterworth to [`Q_MAX`].
pub(crate) fn q_of(res: f64) -> f64 {
    FRAC_1_SQRT_2 * (Q_MAX / FRAC_1_SQRT_2).powf(res.clamp(0.0, 1.0))
}

/// Morph's blend of low-pass, band-pass and high-pass: low-pass to
/// band-pass over the first half, band-pass to high-pass over the second.
pub(crate) fn blend<T>(morph: f64, lp: T, bp: T, hp: T) -> T
where
    T: Mix,
{
    if morph < 0.5 {
        lp.mix(bp, morph * 2.0)
    } else {
        bp.mix(hp, morph * 2.0 - 1.0)
    }
}

/// A value that crossfades, for [`blend`].
pub(crate) trait Mix {
    fn mix(self, to: Self, t: f64) -> Self;
}

impl Mix for f64 {
    fn mix(self, to: f64, t: f64) -> f64 {
        self + (to - self) * t
    }
}

impl Mix for C {
    fn mix(self, to: C, t: f64) -> C {
        self.scale(1.0 - t).add(to.scale(t))
    }
}

/// Softens `x` to within about ±`a`, leaving small values alone.
pub(crate) fn soft(x: f64, a: f64) -> f64 {
    a * (x / a).tanh()
}

/// Drive's saturation: `tanh(d x) / d`, which is `x` itself at `d` = 0.
pub(crate) fn drive(x: f64, d: f64) -> f64 {
    if d < 1e-6 { x } else { (d * x).tanh() / d }
}

/// A value far below the audio's noise floor is left as zero rather than as
/// a denormal.
pub(crate) fn flush(x: f64) -> f64 {
    if x.abs() < 1e-20 { 0.0 } else { x }
}

/// A state-variable section's coefficients.
#[derive(Clone, Copy)]
pub(crate) struct Coef {
    pub(crate) a1: f64,
    pub(crate) a2: f64,
    pub(crate) a3: f64,
    pub(crate) k: f64,
}

impl Coef {
    pub(crate) fn new(g: f64, q: f64) -> Self {
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        Self {
            a1,
            a2: g * a1,
            a3: g * g * a1,
            k,
        }
    }
}

/// A state-variable section's two integrators.
#[derive(Clone, Copy, Default)]
pub(crate) struct Stage {
    pub(crate) ic1: f64,
    pub(crate) ic2: f64,
}

impl Stage {
    /// One sample through the section: its low-pass, raw band-pass and
    /// high-pass outputs.
    pub(crate) fn tick(&mut self, x: f64, c: &Coef, circuit: Circuit) -> [f64; 3] {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);
        match circuit {
            Circuit::Svf => {}
            Circuit::Dfm => {
                self.ic1 = soft(self.ic1, SOFT_LOOSE);
                self.ic2 = soft(self.ic2, SOFT_LOOSE);
            }
            Circuit::Ms2 => self.ic1 = soft(self.ic1, SOFT_TIGHT),
            Circuit::Prd => self.ic2 = soft(self.ic2, SOFT_LOOSE),
        }
        [v2, v1, x - c.k * v1 - v2]
    }
}

/// A ring of past samples, read a fractional number of samples back.
pub(crate) struct Ring {
    pub(crate) samples: Vec<f32>,
    pub(crate) at: usize,
}

impl Ring {
    pub(crate) fn new(len: usize) -> Self {
        Self {
            samples: vec![0.0; len],
            at: 0,
        }
    }

    pub(crate) fn write(&mut self, x: f64) {
        self.samples[self.at] = x as f32;
        self.at = (self.at + 1) % self.samples.len();
    }

    /// The sample `delay` writes ago, between the two nearest.
    pub(crate) fn read(&self, delay: f64) -> f64 {
        let len = self.samples.len();
        let delay = delay.clamp(1.0, (len - 2) as f64);
        let whole = delay.floor();
        let at = |n: usize| f64::from(self.samples[(self.at + len - n) % len]);
        let (a, b) = (at(whole as usize), at(whole as usize + 1));
        a + (b - a) * (delay - whole)
    }
}

/// One channel's filter: every type's state, so a change of type finds
/// them ready.
pub(crate) struct Chan {
    pub(crate) stages: [Stage; 4],
    pub(crate) pole: f64,
    pub(crate) notch: Stage,
    pub(crate) comb_in: Ring,
    pub(crate) comb_out: Ring,
    pub(crate) phase: f64,
    pub(crate) held: f64,
}

impl Chan {
    pub(crate) fn new(rate: f64) -> Self {
        // The longest comb delay: the lowest cutoff, at the highest rate.
        let len = (rate * 2.0 / MIN_HZ).ceil() as usize + 4;
        Self {
            stages: [Stage::default(); 4],
            pole: 0.0,
            notch: Stage::default(),
            comb_in: Ring::new(len),
            comb_out: Ring::new(len),
            phase: 1.0,
            held: 0.0,
        }
    }

    /// Forgets everything the filter has heard, as a new one does.
    pub(crate) fn reset(&mut self) {
        self.stages = [Stage::default(); 4];
        self.pole = 0.0;
        self.notch = Stage::default();
        self.comb_in.samples.fill(0.0);
        self.comb_out.samples.fill(0.0);
        self.phase = 1.0;
        self.held = 0.0;
    }

    /// One sample through the filter `plan` describes, at `rate`.
    pub(crate) fn tick(&mut self, x: f64, plan: &Plan, rate: f64) -> f64 {
        let fc = plan.cutoff();
        let g_of = |hz: f64| (PI * hz.clamp(1.0, rate * 0.49) / rate).tan();
        let q_of = |s: usize| if s == 0 { plan.q } else { FRAC_1_SQRT_2 };
        match plan.kind {
            Kind::LowPass | Kind::HighPass | Kind::BandPass | Kind::Notch | Kind::Morph
                if plan.one_pole =>
            {
                let g = g_of(fc);
                let v = (x - self.pole) * g / (1.0 + g);
                let lp = v + self.pole;
                self.pole = flush(lp + v);
                lp.mix(x - lp, plan.morph)
            }
            Kind::LowPass | Kind::HighPass | Kind::BandPass | Kind::Notch | Kind::Morph => {
                let g = g_of(fc);
                let mut y = x;
                for s in 0..plan.stages {
                    let c = Coef::new(g, q_of(s));
                    let [lp, bp, hp] = self.stages[s].tick(y, &c, plan.circuit);
                    y = match plan.kind {
                        Kind::LowPass => lp,
                        Kind::HighPass => hp,
                        Kind::BandPass => c.k * bp,
                        Kind::Notch => lp + hp,
                        _ => blend(plan.morph, lp, c.k * bp, hp),
                    };
                }
                y
            }
            Kind::NotchLowPass => {
                let c = Coef::new(g_of(plan.notch_hz()), NOTCH_Q);
                let [lp, _, hp] = self.notch.tick(x, &c, Circuit::Svf);
                let mut y = lp + hp;
                let g = g_of(fc);
                for s in 0..plan.stages {
                    y = self.stages[s].tick(y, &Coef::new(g, q_of(s)), Circuit::Svf)[0];
                }
                y
            }
            Kind::Dj => {
                let (high, hz, q) = plan.dj();
                let [lp, _, hp] = self.stages[0].tick(x, &Coef::new(g_of(hz), q), Circuit::Svf);
                if high { hp } else { lp }
            }
            Kind::Comb => {
                let c = plan.feedback();
                let delay = rate / fc.max(1.0);
                let peaks = (1.0 - c) * x + c * self.comb_out.read(delay);
                let notches = (x - c * self.comb_in.read(delay)) / (1.0 + c);
                self.comb_in.write(x);
                self.comb_out.write(peaks);
                peaks.mix(notches, plan.morph)
            }
            Kind::Resampling => {
                self.phase += fc / rate;
                if self.phase >= 1.0 {
                    self.phase -= self.phase.floor();
                    self.held = x;
                }
                self.held
            }
            Kind::Vowel => {
                let mut y = 0.0;
                for (s, (hz, level, q)) in plan.formants().into_iter().enumerate() {
                    let c = Coef::new(g_of(hz), q);
                    y += level * c.k * self.stages[s].tick(x, &c, Circuit::Svf)[1];
                }
                y
            }
        }
    }
}
