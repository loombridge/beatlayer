//! Contour, the native eight-band parametric EQ (WP-2.18a, part 1; the eq
//! spec): eight filters per curve, two curves for the L/R and M/S modes, an
//! optional oversampled path, adaptive Q, and the output spectrum the
//! display draws.
//!
//! Each band is one of six responses, the cut filters at 12 or 48 dB per
//! octave (taken as a cascade of second-order stages), built from the
//! standard bilinear designs (RBJ's audio-EQ cookbook), so every type's
//! magnitude response is its analytic curve. In Stereo mode the first
//! curve's filters run on both channels; in L/R and M/S the first curve
//! runs on the first side (left, or mid) and the second on the other.
//! Adaptive Q raises a gain-bearing band's Q with its boost or cut.
//!
//! Its parameters, ranges, defaults and units are Beatlayer's own reading
//! of Live 12's EQ Eight (see `docs/specs/eq.md`); the manual doesn't
//! record the ranges, the defaults, or what Adaptive Q's law is, so those
//! are Beatlayer's and are listed to observe.

use std::f64::consts::TAU;

use bl_dsp::Oversampler;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Contour's device type.
pub const EQ: &str = "beatlayer.contour";

/// Bands, on each of the two curves.
pub const BANDS: usize = 8;
/// Curves: the first (left in L/R, mid in M/S) and the second (right, side).
pub const CURVES: usize = 2;

/// The mode: one curve on both channels, one per channel, or mid/side.
pub const MODE: ParamId = ParamId(0);
/// Adaptive Q: a gain-bearing band's Q rises with its boost or cut.
pub const ADAPTIVE_Q: ParamId = ParamId(1);
/// Oversample: the filters run at twice the sample rate.
pub const OVERSAMPLE: ParamId = ParamId(2);
/// Analyze: the analyser runs, for the display's spectrum.
pub const ANALYZE: ParamId = ParamId(3);
/// Gain: the output level, after the filters.
pub const GAIN: ParamId = ParamId(4);

/// The mode's choices.
pub const MODES: [&str; 3] = ["Stereo", "L/R", "M/S"];

/// The band parameters, in each band's order.
pub const ON: usize = 0;
/// The band's filter type.
pub const TYPE: usize = 1;
/// The band's frequency.
pub const FREQ: usize = 2;
/// The band's boost or cut, in dB (ignored by the cuts, the notch, and the
/// high cut).
pub const GAIN_SLOT: usize = 3;
/// The band's Q, its resonance.
pub const Q: usize = 4;

/// The band parameters each band carries.
const SLOTS: usize = 5;
/// The parameters before the bands: the mode, the switches, and the gain.
const GLOBALS: u32 = 5;
/// Every parameter the device has.
const PARAM_COUNT: usize = GLOBALS as usize + CURVES * BANDS * SLOTS;

/// The band's filter types, in the chooser's order.
pub const TYPES: [&str; 8] = [
    "Low Cut 48",
    "Low Cut 12",
    "Low Shelf",
    "Peak",
    "Notch",
    "High Shelf",
    "High Cut 12",
    "High Cut 48",
];

/// A band's response: the eight [`TYPES`], by their index.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Type {
    LowCut48,
    LowCut12,
    LowShelf,
    Peak,
    Notch,
    HighShelf,
    HighCut12,
    HighCut48,
}

impl Type {
    /// The eight types, in the chooser's order.
    const ALL: [Self; 8] = [
        Self::LowCut48,
        Self::LowCut12,
        Self::LowShelf,
        Self::Peak,
        Self::Notch,
        Self::HighShelf,
        Self::HighCut12,
        Self::HighCut48,
    ];

    /// The type the chooser's `index` names.
    fn of(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    /// Where the type sits in the chooser.
    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|kind| *kind == self)
            .expect("every type is in the chooser")
    }

    /// How many second-order stages it runs: the 48 dB cut filters are four
    /// in cascade, the rest one.
    fn stages(self) -> usize {
        if matches!(self, Self::LowCut48 | Self::HighCut48) {
            CUT_STAGES
        } else {
            1
        }
    }

    /// Whether it has a gain: the shelves and the peak, as the manual says
    /// (the cuts, the notch, and the high cut have none).
    fn has_gain(self) -> bool {
        matches!(self, Self::LowShelf | Self::Peak | Self::HighShelf)
    }
}

/// The band frequencies' range, in Hz.
const EQ_MIN_HZ: f64 = 20.0;
const EQ_MAX_HZ: f64 = 20_000.0;
/// A band's gain range, in dB (Live's isn't recorded).
const GAIN_RANGE_DB: f64 = 15.0;
/// A band's Q range.
const Q_MIN: f64 = 0.1;
const Q_MAX: f64 = 18.0;
/// The Q a band starts at.
const Q_DEFAULT: f64 = 0.71;

/// A gain-bearing band's Q rises to twice its setting at full gain
/// (Beatlayer's own; Live doesn't record its law), capped at the Q control's
/// top.
const ADAPTIVE_Q_AT_FULL_GAIN: f64 = 2.0;

/// The number of second-order stages each type runs: the 48 dB cut filters
/// are four in cascade, the rest one.
const CUT_STAGES: usize = 4;
/// The most stages a band has.
const MAX_STAGES: usize = CUT_STAGES;

/// The analyser's window, in frames.
const ANALYSER_WINDOW: usize = 1024;
/// The analyser's bands, from [`ANALYSER_MIN_HZ`] to [`ANALYSER_MAX_HZ`].
pub const ANALYSER_BANDS: usize = 64;
const ANALYSER_MIN_HZ: f64 = 20.0;
const ANALYSER_MAX_HZ: f64 = 20_000.0;
/// What the analyser reads for a band with nothing in it, in dB.
const ANALYSER_FLOOR_DB: f32 = -100.0;
/// How often a band's coefficients are worked out, in frames: often enough
/// that a knob's motion is smooth, rarely enough to keep the sin and cos
/// cheap.
const REFRESH_FRAMES: usize = 16;

/// `band`'s (0-based) parameter `slot` on `curve` (0 or 1).
#[must_use]
pub const fn band_param(curve: usize, band: usize, slot: usize) -> ParamId {
    ParamId(GLOBALS + (curve * BANDS * SLOTS + band * SLOTS + slot) as u32)
}

/// The frequency a band starts at: an octave apart, 100 Hz up.
fn default_freq(band: usize) -> f64 {
    100.0 * (1u32 << band) as f64
}

/// The type a band starts at: a low cut first, a high cut last, a peak
/// between (all switched off, so the device starts transparent).
fn default_kind(band: usize) -> Type {
    match band {
        0 => Type::LowCut48,
        7 => Type::HighCut48,
        _ => Type::Peak,
    }
}

/// What a curve is called in its parameters' descriptions.
fn curve_label(curve: usize) -> &'static str {
    if curve == 0 {
        "the first curve (left in L/R, mid in M/S)"
    } else {
        "the second curve (right in L/R, side in M/S)"
    }
}

/// The band's descriptors, in [`ON`] to [`Q`] order.
fn band_descriptors(curve: usize, band: usize) -> [ParamDescriptor; SLOTS] {
    let label = ['A', 'B'][curve];
    let n = band + 1;
    let which = curve_label(curve);
    [
        ParamDescriptor::toggle(
            band_param(curve, band, ON),
            &format!("{label}{n} On"),
            false,
        )
        .describe(&format!("Switches band {n} of {which} on.")),
        ParamDescriptor::choice(
            band_param(curve, band, TYPE),
            &format!("{label}{n} Type"),
            &TYPES,
            default_kind(band).index(),
        )
        .describe(&format!(
            "Band {n}'s response, from the low cut to the high cut."
        )),
        ParamDescriptor::continuous(
            band_param(curve, band, FREQ),
            &format!("{label}{n} Freq"),
            (EQ_MIN_HZ, EQ_MAX_HZ),
            default_freq(band),
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe(&format!("Band {n}'s corner or centre frequency.")),
        ParamDescriptor::continuous(
            band_param(curve, band, GAIN_SLOT),
            &format!("{label}{n} Gain"),
            (-GAIN_RANGE_DB, GAIN_RANGE_DB),
            0.0,
            ParamUnit::Decibels,
        )
        .describe(&format!(
            "Band {n}'s boost or cut; the cuts, the notch, and the high cut ignore it."
        )),
        ParamDescriptor::continuous(
            band_param(curve, band, Q),
            &format!("{label}{n} Q"),
            (Q_MIN, Q_MAX),
            Q_DEFAULT,
            ParamUnit::None,
        )
        .logarithmic()
        .describe(&format!(
            "Band {n}'s resonance, its width around its frequency."
        )),
    ]
}

/// Contour's type: Beatlayer's own name and look, EQ Eight's behaviour.
#[must_use]
pub fn eq_type() -> DeviceType {
    let mut params = vec![
        ParamDescriptor::choice(MODE, "Mode", &MODES, 0).describe(
            "One curve for both channels, one per channel, or one for mid and one for side.",
        ),
        ParamDescriptor::toggle(ADAPTIVE_Q, "Adaptive Q", false)
            .describe("Raises a boosting or cutting band's Q with the size of the boost or cut."),
        ParamDescriptor::toggle(OVERSAMPLE, "Oversample", false)
            .describe("Runs the filters at twice the sample rate, for smoother high frequencies."),
        ParamDescriptor::toggle(ANALYZE, "Analyze", true)
            .describe("Puts the output's spectrum in the display, behind the curves."),
        ParamDescriptor::gain(GAIN, "Gain", 18.0, 0.0)
            .describe("Sets the output level, after the filters."),
    ];
    for curve in 0..CURVES {
        for band in 0..BANDS {
            params.extend(band_descriptors(curve, band));
        }
    }
    DeviceType::native(EQ, "Contour", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

/// Every control of `curve`'s bands, in band order.
fn band_controls(curve: usize) -> Vec<(ParamId, Widget)> {
    (0..BANDS)
        .flat_map(|band| {
            [
                (band_param(curve, band, ON), Widget::Toggle),
                (band_param(curve, band, TYPE), Widget::Chooser),
                (band_param(curve, band, FREQ), Widget::Knob),
                (band_param(curve, band, GAIN_SLOT), Widget::Knob),
                (band_param(curve, band, Q), Widget::Knob),
            ]
        })
        .collect()
}

/// The layout: the globals, the first curve, the display, the second curve.
/// The display grows; the bands are the framework's generic columns, since
/// EQ Eight's own panel (one band at a time, edited in the display) needs
/// the device's own drawing, which the framework doesn't have yet.
fn layout() -> DeviceLayout {
    let globals = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (MODE, Widget::Chooser),
            (ADAPTIVE_Q, Widget::Toggle),
            (OVERSAMPLE, Widget::Toggle),
            (ANALYZE, Widget::Toggle),
            (GAIN, Widget::Knob),
        ],
    );
    let curve = |which: usize| ControlBand::stacked(BODY_HEIGHT, &band_controls(which));
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(globals),
        LayoutNode::Band(curve(0)),
        LayoutNode::Cell(DisplayCell::new("EQ Curve", 200.0, BODY_HEIGHT, 2.0)),
        LayoutNode::Band(curve(1)),
    ]))
}

/// Adds Contour to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, eq_type(), make);
}

/// Makes a Contour for `config`'s rate, channels, and block size. Every
/// buffer it needs is made here, off the audio thread.
#[must_use]
pub fn make(config: &NodeConfig) -> Contour {
    Contour::new(config)
}

/// A biquad's coefficients, normalised so `a0` is 1.
#[derive(Clone, Copy, Default)]
struct Coeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

/// One second-order section: its coefficients and its state, transposed
/// direct form II.
#[derive(Clone, Copy, Default)]
struct Biquad {
    c: Coeffs,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// Its output for `x`, advancing its state.
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = flush(self.c.b0 * x + self.z1);
        self.z1 = flush(self.c.b1 * x - self.c.a1 * y + self.z2);
        self.z2 = flush(self.c.b2 * x - self.c.a2 * y);
        y
    }
}

/// A value far below the audio's noise floor is left as zero rather than as
/// a denormal, whose arithmetic is slow.
fn flush(x: f32) -> f32 {
    if x.abs() < 1e-30 { 0.0 } else { x }
}

/// The coefficient design of `kind` at `freq`, `gain_db`, and `q`, the
/// bilinear designs of RBJ's audio-EQ cookbook, at the rate the stages run.
///
/// A shelf's `q` is the Q of the cookbook's `alpha` in place of its slope
/// `S`, and a cut filter's is its resonance; the 48 dB cuts are four
/// identical stages, so their curve is a plain cascade.
fn coeffs(kind: Type, freq: f64, gain_db: f64, q: f64, rate: f64) -> Coeffs {
    let w0 = TAU * (freq / rate).clamp(1.0e-6, 0.49);
    let (sin, cos) = w0.sin_cos();
    let amplitude = 10f64.powf(gain_db / 40.0);
    let alpha = sin / (2.0 * q.max(Q_MIN));
    let root = amplitude.sqrt();
    let (b0, b1, b2, a0, a1, a2) = match kind {
        Type::LowCut48 | Type::LowCut12 => {
            let half = (1.0 + cos) / 2.0;
            (
                half,
                -(1.0 + cos),
                half,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            )
        }
        Type::LowShelf => (
            amplitude * ((amplitude + 1.0) - (amplitude - 1.0) * cos + 2.0 * root * alpha),
            2.0 * amplitude * ((amplitude - 1.0) - (amplitude + 1.0) * cos),
            amplitude * ((amplitude + 1.0) - (amplitude - 1.0) * cos - 2.0 * root * alpha),
            (amplitude + 1.0) + (amplitude - 1.0) * cos + 2.0 * root * alpha,
            -2.0 * ((amplitude - 1.0) + (amplitude + 1.0) * cos),
            (amplitude + 1.0) + (amplitude - 1.0) * cos - 2.0 * root * alpha,
        ),
        Type::Peak => (
            1.0 + alpha * amplitude,
            -2.0 * cos,
            1.0 - alpha * amplitude,
            1.0 + alpha / amplitude,
            -2.0 * cos,
            1.0 - alpha / amplitude,
        ),
        Type::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
        Type::HighShelf => (
            amplitude * ((amplitude + 1.0) + (amplitude - 1.0) * cos + 2.0 * root * alpha),
            -2.0 * amplitude * ((amplitude - 1.0) + (amplitude + 1.0) * cos),
            amplitude * ((amplitude + 1.0) + (amplitude - 1.0) * cos - 2.0 * root * alpha),
            (amplitude + 1.0) - (amplitude - 1.0) * cos + 2.0 * root * alpha,
            2.0 * ((amplitude - 1.0) - (amplitude + 1.0) * cos),
            (amplitude + 1.0) - (amplitude - 1.0) * cos - 2.0 * root * alpha,
        ),
        Type::HighCut48 | Type::HighCut12 => {
            let half = (1.0 - cos) / 2.0;
            (half, 1.0 - cos, half, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
        }
    };
    Coeffs {
        b0: (b0 / a0) as f32,
        b1: (b1 / a0) as f32,
        b2: (b2 / a0) as f32,
        a1: (a1 / a0) as f32,
        a2: (a2 / a0) as f32,
    }
}

/// One band's filters on one channel.
#[derive(Clone, Copy)]
struct Band {
    stages: [Biquad; MAX_STAGES],
    used: usize,
    on: bool,
    kind: Type,
}

impl Band {
    fn new() -> Self {
        Self {
            stages: [Biquad::default(); MAX_STAGES],
            used: 1,
            on: false,
            kind: Type::Peak,
        }
    }

    /// Redesigns it for this frame's values. A change of type or switch
    /// starts it from silence, so no state of another shape rings on.
    fn design(&mut self, on: bool, kind: Type, freq: f64, gain_db: f64, q: f64, rate: f64) {
        if on != self.on || kind != self.kind {
            self.stages = [Biquad::default(); MAX_STAGES];
            self.on = on;
            self.kind = kind;
        }
        if !on {
            return;
        }
        self.used = kind.stages();
        let c = coeffs(kind, freq, gain_db, q, rate);
        for stage in &mut self.stages[..self.used] {
            stage.c = c;
        }
    }

    /// One sample through it.
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        if !self.on {
            return x;
        }
        let mut y = x;
        for stage in &mut self.stages[..self.used] {
            y = stage.process(y);
        }
        y
    }
}

/// The spectrum of the output, for the display: a window of samples at a
/// time, at [`ANALYSER_BANDS`] logarithmic bands, in dB. Everything it
/// writes it made with, so it doesn't allocate on the audio thread.
struct Analyser {
    /// The samples of the window being filled.
    samples: Vec<f32>,
    filled: usize,
    /// The last complete window's bands, in dB.
    bands: [f32; ANALYSER_BANDS],
    /// The FFT's working buffers and its twiddles.
    re: Vec<f32>,
    im: Vec<f32>,
    twiddle: Vec<(f32, f32)>,
    /// The Hann window's weights.
    weights: Vec<f32>,
    /// Each band's first and last bin.
    edges: [usize; ANALYSER_BANDS + 1],
}

impl Analyser {
    fn new(rate: f64) -> Self {
        let half = ANALYSER_WINDOW / 2;
        let mut edges = [0usize; ANALYSER_BANDS + 1];
        for (band, edge) in edges.iter_mut().enumerate() {
            let ratio =
                (ANALYSER_MAX_HZ / ANALYSER_MIN_HZ).powf(band as f64 / ANALYSER_BANDS as f64);
            *edge = (((ANALYSER_MIN_HZ * ratio) * ANALYSER_WINDOW as f64 / rate).round() as usize)
                .min(half);
        }
        Self {
            samples: vec![0.0; ANALYSER_WINDOW],
            filled: 0,
            bands: [ANALYSER_FLOOR_DB; ANALYSER_BANDS],
            re: vec![0.0; ANALYSER_WINDOW],
            im: vec![0.0; ANALYSER_WINDOW],
            twiddle: (0..ANALYSER_WINDOW / 2)
                .map(|k| {
                    let angle = -TAU * k as f64 / ANALYSER_WINDOW as f64;
                    (angle.cos() as f32, angle.sin() as f32)
                })
                .collect(),
            weights: (0..ANALYSER_WINDOW)
                .map(|i| {
                    let x = i as f64 / ANALYSER_WINDOW as f64;
                    (0.5 - 0.5 * (TAU * x).cos()) as f32
                })
                .collect(),
            edges,
        }
    }

    /// Adds one output sample; a full window is measured and starts again.
    fn push(&mut self, sample: f32) {
        self.samples[self.filled] = sample;
        self.filled += 1;
        if self.filled == ANALYSER_WINDOW {
            self.filled = 0;
            self.measure();
        }
    }

    /// Measures the window into [`bands`](Self::bands).
    fn measure(&mut self) {
        let weights = &self.weights;
        for (i, (sample, weight)) in self.samples.iter().zip(weights).enumerate() {
            self.re[i] = sample * weight;
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im, &self.twiddle);
        // A one-sided spectrum's amplitude: twice the bin over the window's
        // weight, which a full-scale sine reads 0 dB at.
        let window: f32 = weights.iter().sum();
        let scale = 2.0 / window;
        for (band, value) in self.bands.iter_mut().enumerate() {
            // A band whose edges round to one bin reads that bin.
            let low = self.edges[band];
            let high = self.edges[band + 1].max(low + 1);
            let peak = (low..high)
                .map(|bin| self.re[bin].hypot(self.im[bin]))
                .fold(0.0f32, f32::max);
            let amplitude = peak * scale;
            *value = if amplitude > 1.0e-5 {
                20.0 * amplitude.log10()
            } else {
                ANALYSER_FLOOR_DB
            };
        }
    }

    /// The last window's bands, in dB, [`ANALYSER_MIN_HZ`] to
    /// [`ANALYSER_MAX_HZ`], lowest first.
    fn bands(&self) -> &[f32; ANALYSER_BANDS] {
        &self.bands
    }
}

/// An in-place radix-2 FFT of `re` (with `im` zero for real input), the
/// twiddles precomputed.
fn fft(re: &mut [f32], im: &mut [f32], twiddle: &[(f32, f32)]) {
    let n = re.len();
    // The bit-reversal permutation.
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let step = n / len;
        let mut start = 0;
        while start < n {
            for k in 0..len / 2 {
                let (wr, wi) = twiddle[k * step];
                let (a, b) = (start + k, start + k + len / 2);
                let (xr, xi) = (re[a], im[a]);
                let (yr, yi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                re[a] = xr + yr;
                im[a] = xi + yi;
                re[b] = xr - yr;
                im[b] = xi - yi;
            }
            start += len;
        }
        len <<= 1;
    }
}

/// The mode: how the two curves map onto the channels.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The first curve on both channels.
    Stereo,
    /// The first curve on the left, the second on the right.
    LeftRight,
    /// The first curve on the mid, the second on the side.
    MidSide,
}

impl Mode {
    fn of(value: f64) -> Self {
        match value as usize {
            1 => Self::LeftRight,
            2 => Self::MidSide,
            _ => Self::Stereo,
        }
    }
}

/// One stretch's settings, worked out once: every parameter's place in the
/// bank, and what the mode and the bands are.
struct Settings {
    index: [usize; PARAM_COUNT],
    plan: Plan,
}

impl Settings {
    fn new(params: &ParamBank) -> Self {
        let index = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let plan = Plan::new(params, &index);
        Self { index, plan }
    }

    /// A parameter's value at each frame of the stretch.
    fn ramp<'a>(&self, params: &'a ParamBank, id: ParamId) -> &'a [f32] {
        params.ramp(self.index[id.0 as usize])
    }

    /// A parameter's value now.
    fn value(&self, params: &ParamBank, id: ParamId) -> f64 {
        params.value(self.index[id.0 as usize])
    }
}

/// What one stretch's parameters say: the mode, Adaptive Q, and the bands.
struct Plan {
    mode: Mode,
    adaptive: bool,
    /// Per curve, per band: whether it's on and which type it is.
    bands: [[(bool, Type); BANDS]; CURVES],
}

impl Plan {
    fn new(params: &ParamBank, index: &[usize; PARAM_COUNT]) -> Self {
        let value = |id: ParamId| params.value(index[id.0 as usize]);
        let mode = Mode::of(value(MODE));
        let adaptive = value(ADAPTIVE_Q) >= 0.5;
        let mut bands = [[(false, Type::Peak); BANDS]; CURVES];
        for (curve, curve_bands) in bands.iter_mut().enumerate() {
            for (band, setting) in curve_bands.iter_mut().enumerate() {
                let on = value(band_param(curve, band, ON)) >= 0.5;
                let kind = Type::of(value(band_param(curve, band, TYPE)) as usize);
                *setting = (on, kind);
            }
        }
        Self {
            mode,
            adaptive,
            bands,
        }
    }

    /// The curve `channel`'s filters take: in Stereo mode both channels are
    /// the first curve's.
    fn curve(&self, channel: usize) -> usize {
        if self.mode == Mode::Stereo {
            0
        } else {
            channel
        }
    }

    /// The Q a band's filters take this frame, with Adaptive Q's rise.
    fn q(&self, kind: Type, q: f64, gain_db: f64) -> f64 {
        if self.adaptive && kind.has_gain() {
            let rise = 1.0 + (ADAPTIVE_Q_AT_FULL_GAIN - 1.0) * gain_db.abs() / GAIN_RANGE_DB;
            (q * rise).min(Q_MAX)
        } else {
            q
        }
    }
}

/// Contour's audio-thread half.
pub struct Contour {
    rate: f64,
    /// Each channel's eight bands.
    channels: [BandChain; 2],
    /// A 2-times oversampler per channel, for the Oversample switch.
    oversamplers: [Oversampler; 2],
    oversampled: bool,
    latency: u32,
    analyser: Analyser,
}

/// One channel's bands.
struct BandChain {
    bands: [Band; BANDS],
}

impl BandChain {
    fn new() -> Self {
        Self {
            bands: [Band::new(); BANDS],
        }
    }
}

impl Contour {
    /// A Contour for `config`, every buffer made here.
    #[must_use]
    pub fn new(config: &NodeConfig) -> Self {
        let frames = config.max_frames.max(1);
        Self {
            rate: config.sample_rate.as_f64(),
            channels: [BandChain::new(), BandChain::new()],
            oversamplers: [
                Oversampler::new(1, 1, frames),
                Oversampler::new(1, 1, frames),
            ],
            oversampled: false,
            latency: 0,
            analyser: Analyser::new(config.sample_rate.as_f64()),
        }
    }

    /// The analyser's last window, in dB per band ([`ANALYSER_BANDS`] of
    /// them, from 20 Hz to 20 kHz). This is what the display draws, and
    /// what a headless caller reads.
    #[must_use]
    pub fn analyser(&self) -> &[f32; ANALYSER_BANDS] {
        self.analyser.bands()
    }
}

/// Runs `chain`'s filters over `samples`, on `curve`, redesigning each band
/// every [`REFRESH_FRAMES`] frames from the parameter ramps. `factor` is how
/// many samples each frame takes: 1, or 2 through an oversampler.
fn render_chain(
    chain: &mut BandChain,
    curve: usize,
    samples: &mut [f32],
    factor: usize,
    settings: &Settings,
    params: &ParamBank,
    rate: f64,
) {
    for (i, chunk) in samples.chunks_mut(factor).enumerate() {
        if i % REFRESH_FRAMES == 0 {
            for band in 0..BANDS {
                let (on, kind) = settings.plan.bands[curve][band];
                let freq = f64::from(settings.ramp(params, band_param(curve, band, FREQ))[i]);
                let gain = f64::from(settings.ramp(params, band_param(curve, band, GAIN_SLOT))[i]);
                let q = f64::from(settings.ramp(params, band_param(curve, band, Q))[i]);
                // The stages run at `factor` times the rate, so their
                // coefficients are designed for that rate.
                chain.bands[band].design(
                    on,
                    kind,
                    freq,
                    gain,
                    settings.plan.q(kind, q, gain),
                    rate * factor as f64,
                );
            }
        }
        for sample in chunk {
            let mut y = *sample;
            for band in &mut chain.bands {
                y = band.process(y);
            }
            *sample = y;
        }
    }
}

impl NativeDevice for Contour {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let settings = Settings::new(params);
        let plan = &settings.plan;
        let oversample = settings.value(params, OVERSAMPLE) >= 0.5;
        if oversample != self.oversampled {
            self.oversampled = oversample;
            for oversampler in &mut self.oversamplers {
                oversampler.reset();
            }
        }
        self.latency = if oversample {
            self.oversamplers[0].latency()
        } else {
            0
        };

        // 1. To mid/side, if the mode asks for it.
        if plan.mode == Mode::MidSide && channels == 2 {
            let mut iter = audio.channels_mut();
            let left = iter.next().expect("two channels");
            let right = iter.next().expect("two channels");
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let (a, b) = (*l, *r);
                *l = (a + b) * 0.5;
                *r = (a - b) * 0.5;
            }
        }

        // 2. The filters, channel by channel, and the output gain.
        let gain = settings.ramp(params, GAIN);
        for channel in 0..channels {
            let curve = plan.curve(channel);
            let sample = audio.channel_mut(channel);
            {
                let Self {
                    channels,
                    oversamplers,
                    rate,
                    ..
                } = self;
                let chain = &mut channels[channel];
                if oversample {
                    oversamplers[channel].process(0, sample, |high| {
                        render_chain(chain, curve, high, 2, &settings, params, *rate);
                    });
                } else {
                    render_chain(chain, curve, sample, 1, &settings, params, *rate);
                }
            }
        }
        for channel in 0..channels {
            for (sample, level) in audio.channel_mut(channel).iter_mut().zip(gain) {
                *sample *= *level;
            }
        }

        // 3. Back from mid/side.
        if plan.mode == Mode::MidSide && channels == 2 {
            let mut iter = audio.channels_mut();
            let left = iter.next().expect("two channels");
            let right = iter.next().expect("two channels");
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let (mid, side) = (*l, *r);
                *l = mid + side;
                *r = mid - side;
            }
        }

        // 4. The analyser, on the output.
        if settings.value(params, ANALYZE) >= 0.5 {
            if channels == 2 {
                let mut iter = audio.channels_mut();
                let left = iter.next().expect("two channels");
                let right = iter.next().expect("two channels");
                for (l, r) in left.iter().zip(right.iter()) {
                    self.analyser.push((l + r) * 0.5);
                }
            } else {
                for sample in audio.channel_mut(0) {
                    self.analyser.push(*sample);
                }
            }
        }
    }

    fn latency(&self) -> u32 {
        self.latency
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: eq R5
    #[test]
    fn adaptive_q_only_moves_the_gain_bearing_types() {
        let plan = |adaptive: bool| Plan {
            mode: Mode::Stereo,
            adaptive,
            bands: [[(false, Type::Peak); BANDS]; CURVES],
        };
        let flat = plan(false);
        let lifted = plan(true);
        assert!((flat.q(Type::Peak, 1.0, 12.0) - 1.0).abs() < 1e-9);
        assert!(
            (lifted.q(Type::Peak, 1.0, 0.0) - 1.0).abs() < 1e-9,
            "flat at no gain"
        );
        assert!(lifted.q(Type::Peak, 1.0, 12.0) > flat.q(Type::Peak, 1.0, 12.0));
        assert!(
            (lifted.q(Type::Peak, 1.0, -12.0) - lifted.q(Type::Peak, 1.0, 12.0)).abs() < 1e-9,
            "a cut rises as a boost does"
        );
        assert!(
            (lifted.q(Type::Notch, 1.0, 12.0) - 1.0).abs() < 1e-9,
            "no gain to read"
        );
        assert!((lifted.q(Type::LowCut12, 1.0, 0.0) - 1.0).abs() < 1e-9);
    }

    // Spec: eq R2
    #[test]
    fn the_analyser_tables_a_full_scale_sine_near_zero_db() {
        let mut analyser = Analyser::new(48_000.0);
        // 937.5 Hz is bin 20 of the window, so the sine sits on its bin.
        let sine: Vec<f32> = (0..ANALYSER_WINDOW * 2)
            .map(|i| (TAU * 937.5 * i as f64 / 48_000.0).sin() as f32)
            .collect();
        for sample in sine {
            analyser.push(sample);
        }
        let bands = analyser.bands();
        let (loudest, level) = bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .expect("bands");
        let ratio = (ANALYSER_MAX_HZ / ANALYSER_MIN_HZ)
            .powf((loudest as f64 + 0.5) / ANALYSER_BANDS as f64);
        let centre = ANALYSER_MIN_HZ * ratio;
        assert!(
            (600.0..1_600.0).contains(&centre),
            "the peak sits at {centre} Hz, not near 1 kHz"
        );
        assert!(level.abs() < 0.5, "a full-scale sine reads {level} dB");
    }
}
