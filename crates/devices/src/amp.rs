//! Amp, seven amplifier models (WP-8.40; the amp spec): a preamp of triode
//! stages, a tone stack and a power stage with sag and Presence, run at four
//! times the sample rate.
//!
//! Each model is a row of [`SPECS`]: how many preamp stages, the gain range,
//! the triode bias, the tone stack's component values and the sag share. The
//! passive stack is Yeh and Smith's third-order transfer function (DAFx
//! 2006) discretised by the bilinear transform; the Bass model's stack is
//! three cookbook filters. Every value and every voicing is Beatlayer's own.

use bl_dsp::{EnvelopeFollower, Oversampler};
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::eq::{Type, coeffs_f64};
use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Amp's device type.
pub const AMP: &str = "beatlayer.amp";

/// Model, an index into [`MODELS`].
pub const MODEL: ParamId = ParamId(0);
/// Gain, 0 to 10: how hard the preamp is driven.
pub const GAIN: ParamId = ParamId(1);
/// Bass, 0 to 10.
pub const BASS: ParamId = ParamId(2);
/// Middle, 0 to 10.
pub const MIDDLE: ParamId = ParamId(3);
/// Treble, 0 to 10.
pub const TREBLE: ParamId = ParamId(4);
/// Presence, 0 to 10: the power stage's high shelf.
pub const PRESENCE: ParamId = ParamId(5);
/// Volume: how hard the power stage is driven, as a gain.
pub const VOLUME: ParamId = ParamId(6);
/// Dry/Wet, per cent.
pub const DRY_WET: ParamId = ParamId(7);
/// Output: Mono (0) or Dual (1).
pub const OUTPUT: ParamId = ParamId(8);

/// The models, in the order of [`SPECS`].
pub const MODELS: [&str; 7] = ["Clean", "Boost", "Blues", "Rock", "Lead", "Heavy", "Bass"];

/// The oversampler's stages: 4 times the rate, fixed, so a render doesn't
/// depend on the block size or the model.
const STAGES: u32 = 2;
/// Frames between coefficient updates, counted from the device's first
/// frame, so a render doesn't depend on its block size.
const REFRESH_FRAMES: u64 = 16;
/// A model change crossfades over this long.
const FADE_SECONDS: f64 = 0.02;
/// The slope resistor of every passive stack, in ohms.
const SLOPE_R4: f64 = 56_000.0;
/// The high-pass between preamp stages, and the DC blockers after the
/// preamp and the power stage, in Hz.
const COUPLING_HZ: f64 = 20.0;
const DC_HZ: f64 = 5.0;
/// Presence: the shelf's corner, and its gain at 0 and at 10, in dB.
const PRESENCE_HZ: f64 = 3_000.0;
const PRESENCE_DB: (f64, f64) = (-6.0, 9.0);
/// The sag follower's attack and release time constants, in seconds.
const SAG_SECONDS: (f64, f64) = (0.03, 0.3);
/// Volume's scale-back stops at this gain: below it the output falls to
/// silence with Volume (rule G1), above it a clean signal keeps its level.
const VOLUME_FLOOR: f64 = 0.01;
/// The Bass model's stack: corner or centre in Hz, and the gain at 0 and at
/// 10, in dB; bass shelf, middle peak, treble shelf.
const ACTIVE_STACK: [(f64, f64); 3] = [(100.0, 15.0), (600.0, 12.0), (4_000.0, 15.0)];
/// The frequency at which a passive stack's level is made 0 dB at 5, 5, 5.
const MAKEUP_HZ: f64 = 1_000.0;

/// One model of [`SPECS`].
struct Spec {
    /// Triode stages in the preamp.
    stages: usize,
    /// The stage gain k at Gain 0 and at Gain 10.
    k: (f64, f64),
    /// The triode's bias.
    beta: f64,
    /// The passive stack's R1, R2, R3 in ohms and C1, C2, C3 in farads; none
    /// for the active stack.
    stack: Option<[f64; 6]>,
    /// The share the power stage's drive falls by at full output.
    sag: f64,
}

const fn passive(r: [f64; 3], c: [f64; 3]) -> Option<[f64; 6]> {
    Some([
        r[0] * 1e3,
        r[1] * 1e3,
        r[2] * 1e3,
        c[0] * 1e-9,
        c[1] * 1e-9,
        c[2] * 1e-9,
    ])
}

const SPECS: [Spec; 7] = [
    Spec {
        stages: 1,
        k: (1.0, 4.0),
        beta: 0.05,
        stack: passive([250.0, 1000.0, 25.0], [0.25, 20.0, 20.0]),
        sag: 0.05,
    },
    Spec {
        stages: 2,
        k: (2.0, 12.0),
        beta: 0.1,
        stack: passive([220.0, 1000.0, 22.0], [0.47, 22.0, 22.0]),
        sag: 0.1,
    },
    Spec {
        stages: 2,
        k: (2.0, 20.0),
        beta: 0.15,
        stack: passive([250.0, 250.0, 56.0], [0.25, 20.0, 20.0]),
        sag: 0.2,
    },
    Spec {
        stages: 3,
        k: (3.0, 30.0),
        beta: 0.1,
        stack: passive([220.0, 1000.0, 33.0], [0.47, 22.0, 22.0]),
        sag: 0.15,
    },
    Spec {
        stages: 3,
        k: (5.0, 60.0),
        beta: 0.2,
        stack: passive([250.0, 1000.0, 33.0], [0.47, 47.0, 22.0]),
        sag: 0.15,
    },
    Spec {
        stages: 4,
        k: (6.0, 80.0),
        beta: 0.2,
        stack: passive([220.0, 1000.0, 47.0], [0.68, 47.0, 47.0]),
        sag: 0.1,
    },
    Spec {
        stages: 2,
        k: (1.0, 10.0),
        beta: 0.05,
        stack: None,
        sag: 0.2,
    },
];

/// The triode curve (the amp spec, R3), after Koren's triode law: soft and
/// asymmetric.
#[must_use]
pub fn triode(x: f64) -> f64 {
    if x >= 0.0 {
        // (1 + x^1.5)^(2/3) as a cube root squared.
        let root = (1.0 + x * x.sqrt()).cbrt();
        1.5 * x / (root * root)
    } else {
        1.2 * (x / 1.2).tanh() - 0.08 * x * x / (1.0 + x * x)
    }
}

/// A value far below the audio's noise floor is left as zero rather than as
/// a denormal.
fn flush(x: f64) -> f64 {
    if x.abs() < 1e-30 { 0.0 } else { x }
}

/// A filter of up to third order in transposed direct form II, in f64: the
/// passive stack's poles sit near z = 1 at 4 times the rate.
#[derive(Clone, Copy)]
struct Iir {
    /// `[b0, b1, b2, b3]`.
    b: [f64; 4],
    /// `[a1, a2, a3]`.
    a: [f64; 3],
    z: [f64; 3],
}

impl Iir {
    const PASS: Self = Self {
        b: [1.0, 0.0, 0.0, 0.0],
        a: [0.0; 3],
        z: [0.0; 3],
    };

    /// A one-pole high-pass at `hz`.
    fn high_pass(hz: f64, rate: f64) -> Self {
        let pole = (-std::f64::consts::TAU * hz / rate).exp();
        Self {
            b: [1.0, -1.0, 0.0, 0.0],
            a: [-pole, 0.0, 0.0],
            z: [0.0; 3],
        }
    }

    /// A cookbook section.
    fn section(kind: Type, freq: f64, gain_db: f64, q: f64, rate: f64) -> Self {
        let [b0, b1, b2, a1, a2] = coeffs_f64(kind, freq, gain_db, q, rate);
        Self {
            b: [b0, b1, b2, 0.0],
            a: [a1, a2, 0.0],
            z: [0.0; 3],
        }
    }

    /// Takes `other`'s coefficients and keeps the state.
    fn design(&mut self, other: Self) {
        self.b = other.b;
        self.a = other.a;
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = flush(self.b[1] * x - self.a[0] * y + self.z[1]);
        self.z[1] = flush(self.b[2] * x - self.a[1] * y + self.z[2]);
        self.z[2] = flush(self.b[3] * x - self.a[2] * y);
        y
    }
}

/// The passive stack's analog transfer function (Yeh and Smith): the
/// numerator `b1 s + b2 s² + b3 s³` over `1 + a1 s + a2 s² + a3 s³`, as
/// `([0, b1, b2, b3], [1, a1, a2, a3])`, for the treble pot at `t`, the mid
/// pot at `m` and the bass pot at `l`, each 0 to 1.
fn analog_stack(values: &[f64; 6], t: f64, m: f64, l: f64) -> ([f64; 4], [f64; 4]) {
    let [r1, r2, r3, c1, c2, c3] = *values;
    let r4 = SLOPE_R4;
    let c123 = c1 * c2 * c3;
    let b1 = t * c1 * r1 + m * c3 * r3 + l * (c1 * r2 + c2 * r2) + (c1 * r3 + c2 * r3);
    let b2 = t * (c1 * c2 * r1 * r4 + c1 * c3 * r1 * r4)
        - m * m * (c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
        + m * (c1 * c3 * r1 * r3 + c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
        + l * (c1 * c2 * r1 * r2 + c1 * c2 * r2 * r4 + c1 * c3 * r2 * r4)
        + l * m * (c1 * c3 * r2 * r3 + c2 * c3 * r2 * r3)
        + (c1 * c2 * r1 * r3 + c1 * c2 * r3 * r4 + c1 * c3 * r3 * r4);
    let b3 = l * m * c123 * (r1 * r2 * r3 + r2 * r3 * r4)
        - m * m * c123 * (r1 * r3 * r3 + r3 * r3 * r4)
        + m * c123 * (r1 * r3 * r3 + r3 * r3 * r4)
        + t * c123 * r1 * r3 * r4
        - t * m * c123 * r1 * r3 * r4
        + t * l * c123 * r1 * r2 * r4;
    let a1 =
        (c1 * r1 + c1 * r3 + c2 * r3 + c2 * r4 + c3 * r4) + m * c3 * r3 + l * (c1 * r2 + c2 * r2);
    let a2 = m * (c1 * c3 * r1 * r3 - c2 * c3 * r3 * r4 + c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
        + l * m * (c1 * c3 * r2 * r3 + c2 * c3 * r2 * r3)
        - m * m * (c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
        + l * (c1 * c2 * r2 * r4 + c1 * c2 * r1 * r2 + c1 * c3 * r2 * r4 + c2 * c3 * r2 * r4)
        + (c1 * c2 * r1 * r4
            + c1 * c3 * r1 * r4
            + c1 * c2 * r3 * r4
            + c1 * c2 * r1 * r3
            + c1 * c3 * r3 * r4
            + c2 * c3 * r3 * r4);
    let a3 = l * m * c123 * (r1 * r2 * r3 + r2 * r3 * r4)
        - m * m * c123 * (r1 * r3 * r3 + r3 * r3 * r4)
        + m * c123 * (r3 * r3 * r4 + r1 * r3 * r3 - r1 * r3 * r4)
        + l * c123 * r1 * r2 * r4
        + c123 * r1 * r3 * r4;
    ([0.0, b1, b2, b3], [1.0, a1, a2, a3])
}

/// The knob positions of a passive stack: Treble and Middle are linear, Bass
/// is an audio taper.
fn pots(bass: f64, middle: f64, treble: f64) -> (f64, f64, f64) {
    (
        treble / 10.0,
        middle / 10.0,
        ((bass / 10.0 - 1.0) * 3.4).exp(),
    )
}

/// The stack's gain that makes its level at [`MAKEUP_HZ`] 0 dB with every
/// knob at 5: a passive stack loses about 20 dB there.
fn makeup(values: &[f64; 6]) -> f64 {
    let (t, m, l) = pots(5.0, 5.0, 5.0);
    let (b, a) = analog_stack(values, t, m, l);
    let w = std::f64::consts::TAU * MAKEUP_HZ;
    let eval = |c: &[f64; 4]| {
        // c0 + c1 s + c2 s² + c3 s³ at s = jw, by Horner's rule.
        let (mut re, mut im) = (0.0, 0.0);
        for coef in c.iter().rev() {
            (re, im) = (-w * im + coef, w * re);
        }
        re.hypot(im)
    };
    eval(&a) / eval(&b)
}

/// The passive stack at `rate` by the bilinear transform: s is
/// `2 rate (1 − z⁻¹) / (1 + z⁻¹)`, and each term of the polynomials is
/// scaled by (1 + z⁻¹)³.
fn passive_stack(values: &[f64; 6], bass: f64, middle: f64, treble: f64, rate: f64) -> Iir {
    let (t, m, l) = pots(bass, middle, treble);
    let (b, a) = analog_stack(values, t, m, l);
    let c = 2.0 * rate;
    // (1 − z)^k (1 + z)^(3 − k) for k = 0 to 3, in powers of z = z⁻¹.
    const TERMS: [[f64; 4]; 4] = [
        [1.0, 3.0, 3.0, 1.0],
        [1.0, 1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0, 1.0],
        [1.0, -3.0, 3.0, -1.0],
    ];
    let expand = |p: &[f64; 4]| {
        let mut out = [0.0; 4];
        for (k, term) in TERMS.iter().enumerate() {
            for (o, t) in out.iter_mut().zip(term) {
                *o += p[k] * c.powi(k as i32) * t;
            }
        }
        out
    };
    let (num, den) = (expand(&b), expand(&a));
    Iir {
        b: num.map(|v| v / den[0]),
        a: [den[1] / den[0], den[2] / den[0], den[3] / den[0]],
        z: [0.0; 3],
    }
}

/// The Presence shelf's gain at `presence`, in dB.
fn presence_db(presence: f64) -> f64 {
    PRESENCE_DB.0 + (PRESENCE_DB.1 - PRESENCE_DB.0) * presence / 10.0
}

/// The Amp device type.
#[must_use]
pub fn amp_type() -> DeviceType {
    let knob = |id, name: &str, default| {
        ParamDescriptor::continuous(id, name, (0.0, 10.0), default, ParamUnit::None)
    };
    let params = vec![
        ParamDescriptor::choice(MODEL, "Model", &MODELS, 0).describe(
            "The amplifier: Clean to Heavy, with Bass for bass guitar and other low sources.",
        ),
        knob(GAIN, "Gain", 5.0).describe("How hard the preamp is driven; more is dirtier."),
        knob(BASS, "Bass", 5.0).describe("The tone stack's low end."),
        knob(MIDDLE, "Middle", 5.0).describe("The tone stack's mids."),
        knob(TREBLE, "Treble", 5.0).describe("The tone stack's high end."),
        knob(PRESENCE, "Presence", 5.0)
            .describe("Brightens or darkens the top end in the power stage."),
        ParamDescriptor::gain(VOLUME, "Volume", 12.0, 0.0).describe(
            "How hard the power stage is driven; a clean signal keeps its level. At the bottom \
             the amp is silent.",
        ),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the dry signal and the amp."),
        ParamDescriptor::choice(OUTPUT, "Output", &["Mono", "Dual"], 0).describe(
            "Mono sums the channels and runs one amp; Dual runs an amp on each channel, which \
             takes about twice the processing.",
        ),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![LayoutNode::Band(
        ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (MODEL, Widget::Chooser),
                (GAIN, Widget::Knob),
                (BASS, Widget::Knob),
                (MIDDLE, Widget::Knob),
                (TREBLE, Widget::Knob),
                (PRESENCE, Widget::Knob),
                (VOLUME, Widget::Knob),
                (OUTPUT, Widget::Chooser),
                (DRY_WET, Widget::Knob),
            ],
        ),
    )]));
    DeviceType::native(AMP, "Amp", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[
                (
                    "Amp",
                    &[
                        "Model", "Gain", "Bass", "Middle", "Treble", "Presence", "Volume",
                        "Dry/Wet",
                    ],
                ),
                ("Output", &["Output"]),
            ]),
    )
}

/// Adds Amp to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, amp_type(), make);
}

/// Makes an Amp for `device`'s model and `config`'s rate, channels and block
/// size. Every buffer is made here, off the audio thread.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> Amp {
    Amp::new(device, config)
}

/// What a voice needs of a frame's parameters.
struct Frame {
    /// Each preamp stage's gain.
    k: f64,
    /// The power stage's drive before sag, and its scale-back.
    volume: f64,
    comp: f64,
}

/// One model's chain on one channel, at 4 times the rate.
#[derive(Clone)]
struct Voice {
    model: usize,
    /// Between the preamp's stages.
    coupling: [Iir; 3],
    /// After the preamp, and after the power stage.
    dc: [Iir; 2],
    /// The stack: one passive section, or three active ones.
    stack: [Iir; 3],
    makeup: f64,
    presence: Iir,
    follower: EnvelopeFollower,
    /// The sag follower's level.
    sag: f64,
    /// Whether its filters wait for a design from the knobs.
    fresh: bool,
    rate: f64,
}

impl Voice {
    fn new(model: usize, rate: f64) -> Self {
        let mut follower = EnvelopeFollower::new(rate);
        follower.set(SAG_SECONDS.0, SAG_SECONDS.1, false);
        Self {
            model,
            coupling: [Iir::high_pass(COUPLING_HZ, rate); 3],
            dc: [Iir::high_pass(DC_HZ, rate); 2],
            stack: [Iir::PASS; 3],
            makeup: SPECS[model].stack.as_ref().map_or(1.0, makeup),
            presence: Iir::PASS,
            follower,
            sag: 0.0,
            fresh: true,
            rate,
        }
    }

    /// Redesigns the tone stack and Presence for the knobs.
    fn design(&mut self, bass: f64, middle: f64, treble: f64, presence: f64) {
        self.fresh = false;
        let rate = self.rate;
        match &SPECS[self.model].stack {
            Some(values) => {
                self.stack[0].design(passive_stack(values, bass, middle, treble, rate));
            }
            None => {
                let kinds = [Type::LowShelf, Type::Peak, Type::HighShelf];
                for (i, kind) in kinds.into_iter().enumerate() {
                    let (freq, range) = ACTIVE_STACK[i];
                    let knob = [bass, middle, treble][i];
                    let q = if i == 1 { 0.7 } else { 0.71 };
                    self.stack[i].design(Iir::section(
                        kind,
                        freq,
                        range * (knob - 5.0) / 5.0,
                        q,
                        rate,
                    ));
                }
            }
        }
        self.presence.design(Iir::section(
            Type::HighShelf,
            PRESENCE_HZ,
            presence_db(presence),
            0.71,
            rate,
        ));
    }

    /// One sample through preamp, tone stack and power stage.
    #[inline]
    fn tick(&mut self, x: f64, frame: &Frame) -> f64 {
        let spec = &SPECS[self.model];
        let bias = triode(spec.beta);
        let mut u = x;
        for stage in 0..spec.stages {
            u = triode(frame.k * u + spec.beta) - bias;
            if stage + 1 < spec.stages {
                u = self.coupling[stage].process(u);
            }
        }
        u = self.dc[0].process(u) * self.makeup;
        for section in &mut self.stack {
            u = section.process(u);
        }
        u = self.presence.process(u);
        let drive = frame.volume * (1.0 - spec.sag * self.sag.min(1.0));
        let y = (drive * u).tanh();
        self.sag = self.follower.process(y as f32);
        self.dc[1].process(y * frame.comp)
    }
}

/// The values the audio thread's loops read: a stretch's parameters.
struct Run<'a> {
    ticks: u64,
    /// Frames since a model change began, and which voice is the new one.
    fade_done: u64,
    fade_frames: u64,
    active: usize,
    factor: usize,
    gain: &'a [f32],
    bass: &'a [f32],
    middle: &'a [f32],
    treble: &'a [f32],
    presence: &'a [f32],
    volume: &'a [f32],
}

impl Run<'_> {
    /// Runs `voices` over `high`, the channel at 4 times the rate, from base
    /// frame `first` of the stretch.
    fn channel(&self, voices: &mut [Voice; 2], high: &mut [f32], first: usize) {
        for (j, quad) in high.chunks_exact_mut(self.factor).enumerate() {
            let i = first + j;
            let refresh = (self.ticks + i as u64).is_multiple_of(REFRESH_FRAMES);
            let fade = self.fade_done + i as u64;
            let fading = fade < self.fade_frames;
            let mix = if fading {
                fade as f64 / self.fade_frames as f64
            } else {
                1.0
            };
            let volume = f64::from(self.volume[i]);
            let comp = 1.0 / volume.max(VOLUME_FLOOR);
            let mut out = [0.0f64; 4];
            for (slot, voice) in voices.iter_mut().enumerate() {
                if slot != self.active && !fading {
                    continue;
                }
                if refresh || voice.fresh {
                    voice.design(
                        f64::from(self.bass[i]),
                        f64::from(self.middle[i]),
                        f64::from(self.treble[i]),
                        f64::from(self.presence[i]),
                    );
                }
                let (lo, hi) = SPECS[voice.model].k;
                let frame = Frame {
                    k: lo * (hi / lo).powf(f64::from(self.gain[i]) / 10.0),
                    volume,
                    comp,
                };
                let weight = if slot == self.active { mix } else { 1.0 - mix };
                for (sample, out) in quad.iter().zip(&mut out) {
                    *out += weight * voice.tick(f64::from(*sample), &frame);
                }
            }
            for (sample, out) in quad.iter_mut().zip(out) {
                *sample = out as f32;
            }
        }
    }
}

/// Amp's audio-thread half.
pub struct Amp {
    rate: f64,
    /// The model now, and per channel its voice and the one fading out.
    model: usize,
    voices: [[Voice; 2]; 2],
    active: usize,
    fade_done: u64,
    fade_frames: u64,
    oversamplers: [Oversampler; 2],
    /// The signal on its way through the amp, and the dry signal, delayed
    /// by the oversampler's latency, per channel.
    work: [Vec<f32>; 2],
    dry: [Vec<f32>; 2],
    delay: [Vec<f32>; 2],
    delay_at: usize,
    /// Frames processed so far, which the coefficient refresh counts.
    ticks: u64,
    latency: u32,
}

impl Amp {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let frames = config.max_frames.max(1);
        let oversampler = || Oversampler::new(STAGES, config.channels.max(1), frames);
        let oversamplers = [oversampler(), oversampler()];
        let latency = oversamplers[0].latency();
        let model = (device.params.get(&MODEL).copied().unwrap_or(0.0).max(0.0) as usize)
            .min(MODELS.len() - 1);
        let fade_frames = (FADE_SECONDS * rate) as u64;
        let voice = Voice::new(model, rate * f64::from(1u32 << STAGES));
        Self {
            rate,
            model,
            voices: std::array::from_fn(|_| [voice.clone(), voice.clone()]),
            active: 0,
            // No fade at the start.
            fade_done: fade_frames,
            fade_frames,
            oversamplers,
            work: [vec![0.0; frames], vec![0.0; frames]],
            dry: [vec![0.0; frames], vec![0.0; frames]],
            delay: [vec![0.0; latency as usize], vec![0.0; latency as usize]],
            delay_at: 0,
            ticks: 0,
            latency,
        }
    }
}

impl NativeDevice for Amp {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let model = (params.value(MODEL.0 as usize) as usize).min(MODELS.len() - 1);
        let dual = params.value(OUTPUT.0 as usize) >= 0.5;
        // shortcut: a second change inside a crossfade cuts the voice that
        // was fading out; a smoother cut needs a third voice.
        if model != self.model {
            self.model = model;
            self.active = 1 - self.active;
            self.fade_done = 0;
            let rate = self.rate * f64::from(1u32 << STAGES);
            for voices in &mut self.voices {
                voices[self.active] = Voice::new(model, rate);
            }
        }
        let ramp = |id: ParamId| params.ramp(id.0 as usize);
        let run = Run {
            ticks: self.ticks,
            fade_done: self.fade_done,
            fade_frames: self.fade_frames,
            active: self.active,
            factor: self.oversamplers[0].factor(),
            gain: ramp(GAIN),
            bass: ramp(BASS),
            middle: ramp(MIDDLE),
            treble: ramp(TREBLE),
            presence: ramp(PRESENCE),
            volume: ramp(VOLUME),
        };
        let mix = ramp(DRY_WET);

        // 1. The dry signal, delayed, and the signal to run: each channel's
        // own, or with Mono the channels' mean.
        for channel in 0..channels {
            let delay = &mut self.delay[channel];
            let input = audio.channel(channel);
            for (i, (dry, x)) in self.dry[channel].iter_mut().zip(input).enumerate() {
                let at = (self.delay_at + i) % delay.len();
                *dry = std::mem::replace(&mut delay[at], *x);
            }
        }
        self.delay_at = (self.delay_at + frames) % self.delay[0].len();
        let runs = if dual { channels } else { 1 };
        for channel in 0..runs {
            let work = &mut self.work[channel][..frames];
            work.copy_from_slice(audio.channel(channel));
            if !dual && channels == 2 {
                for (w, r) in work.iter_mut().zip(audio.channel(1)) {
                    *w = 0.5 * (*w + r);
                }
            }
        }

        // 2. The amp, at 4 times the rate.
        for channel in 0..runs {
            let voices = &mut self.voices[channel];
            let mut first = 0;
            let factor = run.factor;
            self.oversamplers[channel].process(
                channel,
                &mut self.work[channel][..frames],
                |high| {
                    run.channel(voices, high, first);
                    first += high.len() / factor;
                },
            );
        }

        // 3. Dry/Wet.
        for channel in 0..channels {
            let wet = &self.work[if dual { channel } else { 0 }];
            let out = audio.channel_mut(channel);
            for (((out, dry), wet), mix) in out.iter_mut().zip(&self.dry[channel]).zip(wet).zip(mix)
            {
                let w = mix / 100.0;
                *out = dry * (1.0 - w) + wet * w;
            }
        }
        self.ticks += frames as u64;
        self.fade_done = (self.fade_done + frames as u64).min(self.fade_frames);
    }

    fn latency(&self) -> u32 {
        self.latency
    }
}
