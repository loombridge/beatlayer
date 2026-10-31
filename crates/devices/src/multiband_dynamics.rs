//! Multiband Dynamics, the native multiband dynamics device (WP-5.22; the
//! multiband-dynamics spec): up to three bands split by Linkwitz-Riley
//! crossovers, each with an Above and a Below threshold and ratio, so a band
//! compresses or expands, downward or upward, on each side of its level.
//!
//! A band's detector hears the band's own signal after its Input gain, or
//! the same band of the device's sidechain key. The two sides each turn the
//! detector's level into a gain in dB (the static curve, over an optional
//! soft knee), and each smooths that gain with its own Attack and Release.
//! The band's output is its signal through the two sides' gains, its Output
//! gain and its solo. The crossovers sum to an all-pass, so with every ratio
//! at 1 the device changes only the signal's phase, and they add no latency.
//!
//! Its ranges and defaults are Beatlayer's own reading of Live 12's
//! Multiband Dynamics (see `docs/specs/multiband-dynamics.md`); the manual
//! gives only the 500 Hz and 2 kHz example crossovers, so those are listed to
//! observe.

use core::f64::consts::{FRAC_1_SQRT_2, TAU};

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::db_to_gain;
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker,
    DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Multiband Dynamics's device type.
pub const MULTIBAND_DYNAMICS: &str = "beatlayer.triband";

/// Low: whether the low band exists. With High off too, one band remains.
pub const LOW_ON: ParamId = ParamId(0);
/// High: whether the high band exists.
pub const HIGH_ON: ParamId = ParamId(1);
/// Low Freq: the crossover between the low and mid bands, in Hz.
pub const LOW_FREQ: ParamId = ParamId(2);
/// High Freq: the crossover between the mid and high bands, in Hz.
pub const HIGH_FREQ: ParamId = ParamId(3);
/// Soft Knee: whether the thresholds turn gradually.
pub const SOFT_KNEE: ParamId = ParamId(4);
/// Detector: Peak or RMS.
pub const DETECTOR: ParamId = ParamId(5);
/// Time: scales every attack and release, in per cent.
pub const TIME: ParamId = ParamId(6);
/// Amount: scales every ratio toward 1, in per cent.
pub const AMOUNT: ParamId = ParamId(7);
/// Output: the device's output gain.
pub const OUTPUT: ParamId = ParamId(8);

/// The bands, by their indices.
pub const LOW: usize = 0;
pub const MID: usize = 1;
pub const HIGH: usize = 2;
pub(crate) const BANDS: usize = 3;
pub(crate) const BAND_NAMES: [&str; BANDS] = ["Low", "Mid", "High"];

/// A band's parameters, as offsets from its first ([`band_param`]).
pub const ACTIVE: u32 = 0;
pub const SOLO: u32 = 1;
pub const INPUT: u32 = 2;
pub const BAND_OUTPUT: u32 = 3;
pub const ABOVE_THRESHOLD: u32 = 4;
pub const ABOVE_RATIO: u32 = 5;
pub const ABOVE_ATTACK: u32 = 6;
pub const ABOVE_RELEASE: u32 = 7;
pub const BELOW_THRESHOLD: u32 = 8;
pub const BELOW_RATIO: u32 = 9;
pub const BELOW_ATTACK: u32 = 10;
pub const BELOW_RELEASE: u32 = 11;
const FIELDS: u32 = 12;

/// Where the bands' parameters start, and how far apart they are.
const BANDS_FROM: u32 = 16;
const BAND_STRIDE: u32 = 16;

/// Band `band`'s parameter `field` ([`ACTIVE`] and the others).
#[must_use]
pub const fn band_param(band: usize, field: u32) -> ParamId {
    ParamId(BANDS_FROM + band as u32 * BAND_STRIDE + field)
}

/// The Detector choices, by their indices.
const DETECTOR_RMS: usize = 1;

/// The soft knee's width, in dB, centred on each threshold. Beatlayer's own.
const KNEE_DB: f64 = 6.0;

/// The lowest level the curve reads, in dB.
const FLOOR_DB: f64 = -120.0;

/// The deepest cut and the highest boost one side asks for, in dB.
/// Beatlayer's own.
const MIN_GAIN_DB: f64 = -120.0;
const MAX_BOOST_DB: f64 = 24.0;

/// The RMS detector's averaging time, in milliseconds (as Compressor's).
const RMS_MS: f64 = 10.0;

/// The thresholds' range, in dB.
const THRESHOLD_RANGE: (f64, f64) = (-70.0, 0.0);

/// The ratios' range: below 1 a side expands (above) or compresses toward
/// silence's opposite (below); above 1 it compresses.
const RATIO_RANGE: (f64, f64) = (0.25, 100.0);

/// The views the display offers, the first the one it starts in (the
/// manual's T, B and A buttons: Time, Below and Above).
pub const VIEWS: [&str; 3] = ["Time", "Below", "Above"];

/// The device type.
#[must_use]
pub fn multiband_dynamics_type() -> DeviceType {
    let mut params = vec![
        ParamDescriptor::toggle(LOW_ON, "Low", true)
            .describe("Whether the low band exists. With High off too, only the mid band does."),
        ParamDescriptor::toggle(HIGH_ON, "High", true)
            .describe("Whether the high band exists. With Low off too, only the mid band does."),
        ParamDescriptor::continuous(
            LOW_FREQ,
            "Low Freq",
            (20.0, 20_000.0),
            500.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The crossover between the low and mid bands."),
        ParamDescriptor::continuous(
            HIGH_FREQ,
            "High Freq",
            (20.0, 20_000.0),
            2000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The crossover between the mid and high bands; never below the low one."),
        ParamDescriptor::toggle(SOFT_KNEE, "Soft Knee", false)
            .describe("Makes the thresholds take hold gradually."),
        ParamDescriptor::choice(DETECTOR, "Detector", &["Peak", "RMS"], 0).describe(
            "Whether the detectors follow short peaks, or an RMS level that short peaks \
             barely move.",
        ),
        ParamDescriptor::continuous(TIME, "Time", (10.0, 1000.0), 100.0, ParamUnit::Percent)
            .logarithmic()
            .describe("Scales every attack and release together."),
        ParamDescriptor::continuous(AMOUNT, "Amount", (0.0, 200.0), 100.0, ParamUnit::Percent)
            .describe("Scales every ratio toward 1; at 0 % no band is changed."),
        ParamDescriptor::gain(OUTPUT, "Output", 12.0, 0.0)
            .describe("The device's output level, down to silence."),
    ];
    for band in 0..BANDS {
        params.extend(band_descriptors(band));
    }
    DeviceType::native(
        MULTIBAND_DYNAMICS,
        "Multiband Dynamics",
        DeviceRole::AudioEffect,
    )
    .with_sidechain()
    .with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_views(&VIEWS)
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Low Freq",
                        "High Freq",
                        "Soft Knee",
                        "Detector",
                        "Time",
                        "Amount",
                        "Output",
                    ],
                ),
                (
                    "Bands",
                    &[
                        "Low",
                        "High",
                        "Low Active",
                        "Low Solo",
                        "Mid Active",
                        "Mid Solo",
                        "High Active",
                        "High Solo",
                    ],
                ),
                (
                    "Low Above",
                    &[
                        "Low Input",
                        "Low Output",
                        "Low Above Threshold",
                        "Low Above Ratio",
                        "Low Above Attack",
                        "Low Above Release",
                    ],
                ),
                (
                    "Low Below",
                    &[
                        "Low Below Threshold",
                        "Low Below Ratio",
                        "Low Below Attack",
                        "Low Below Release",
                    ],
                ),
                (
                    "Mid Above",
                    &[
                        "Mid Input",
                        "Mid Output",
                        "Mid Above Threshold",
                        "Mid Above Ratio",
                        "Mid Above Attack",
                        "Mid Above Release",
                    ],
                ),
                (
                    "Mid Below",
                    &[
                        "Mid Below Threshold",
                        "Mid Below Ratio",
                        "Mid Below Attack",
                        "Mid Below Release",
                    ],
                ),
                (
                    "High Above",
                    &[
                        "High Input",
                        "High Output",
                        "High Above Threshold",
                        "High Above Ratio",
                        "High Above Attack",
                        "High Above Release",
                    ],
                ),
                (
                    "High Below",
                    &[
                        "High Below Threshold",
                        "High Below Ratio",
                        "High Below Attack",
                        "High Below Release",
                    ],
                ),
            ]),
    )
}

/// One band's descriptors, in [`ACTIVE`] to [`BELOW_RELEASE`] order.
fn band_descriptors(band: usize) -> Vec<ParamDescriptor> {
    let name = |what: &str| format!("{} {what}", BAND_NAMES[band]);
    let param = |field| band_param(band, field);
    let side = |above: bool| {
        let (side, first, threshold) = if above {
            ("Above", ABOVE_THRESHOLD, -12.0)
        } else {
            ("Below", BELOW_THRESHOLD, -48.0)
        };
        [
            ParamDescriptor::continuous(
                param(first),
                &name(&format!("{side} Threshold")),
                THRESHOLD_RANGE,
                threshold,
                ParamUnit::Decibels,
            )
            .describe(if above {
                "The level above which this band is compressed or expanded."
            } else {
                "The level below which this band is compressed or expanded."
            }),
            ParamDescriptor::continuous(
                param(first + 1),
                &name(&format!("{side} Ratio")),
                RATIO_RANGE,
                1.0,
                ParamUnit::Ratio,
            )
            .logarithmic()
            .describe(if above {
                "Above 1 compresses the levels over the threshold downward; below 1 expands \
                 them upward."
            } else {
                "Above 1 compresses the levels under the threshold upward; below 1 expands \
                 them downward."
            }),
            ParamDescriptor::continuous(
                param(first + 2),
                &name(&format!("{side} Attack")),
                (0.1, 500.0),
                10.0,
                ParamUnit::Milliseconds,
            )
            .logarithmic()
            .describe("How long this side takes to reach its full effect."),
            ParamDescriptor::continuous(
                param(first + 3),
                &name(&format!("{side} Release")),
                (1.0, 5000.0),
                100.0,
                ParamUnit::Milliseconds,
            )
            .logarithmic()
            .describe("How long this side takes to let go."),
        ]
    };
    let mut out = vec![
        ParamDescriptor::toggle(param(ACTIVE), &name("Active"), true)
            .describe("Whether the band is processed; off, it passes its input as it is."),
        ParamDescriptor::toggle(param(SOLO), &name("Solo"), false)
            .describe("Mutes the bands that aren't soloed."),
        ParamDescriptor::gain(param(INPUT), &name("Input"), 15.0, 0.0)
            .describe("The band's level before its dynamics."),
        ParamDescriptor::gain(param(BAND_OUTPUT), &name("Output"), 15.0, 0.0)
            .describe("The band's level after its dynamics."),
    ];
    out.extend(side(true));
    out.extend(side(false));
    out
}

/// The device's layout: the crossovers and band switches, the display, the
/// three bands' controls, and the global controls.
fn layout() -> DeviceLayout {
    let band = |band: usize| {
        let controls: Vec<_> = (0..FIELDS)
            .map(|field| {
                let widget = match field {
                    ACTIVE | SOLO => Widget::Toggle,
                    _ => Widget::Knob,
                };
                (band_param(band, field), widget)
            })
            .collect();
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, &controls))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (LOW_ON, Widget::Toggle),
                (HIGH_ON, Widget::Toggle),
                (LOW_FREQ, Widget::Knob),
                (HIGH_FREQ, Widget::Knob),
            ],
        )),
        LayoutNode::Cell(DisplayCell::new("Bands", 200.0, BODY_HEIGHT, 1.0)),
        band(LOW),
        band(MID),
        band(HIGH),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (SOFT_KNEE, Widget::Toggle),
                (DETECTOR, Widget::Chooser),
                (TIME, Widget::Knob),
                (AMOUNT, Widget::Knob),
                (OUTPUT, Widget::Knob),
            ],
        )),
    ]))
}

/// Adds Multiband Dynamics to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, multiband_dynamics_type(), |_, config| {
        Triband::new(config)
    });
}

/// A second-order section in transposed direct form II, in `f64` so the
/// crossovers keep their precision down to 20 Hz.
#[derive(Clone, Copy, Default)]
struct Section {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Section {
    /// A Butterworth low-pass or high-pass at `hz` (RBJ); `hz` is held just
    /// below Nyquist.
    fn set(&mut self, high_pass: bool, hz: f64, rate: f64) {
        let w = TAU * hz.min(rate * 0.49) / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let k = if high_pass { 1.0 + cos } else { 1.0 - cos };
        self.b0 = 0.5 * k / a0;
        self.b1 = if high_pass { -k } else { k } / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// A Linkwitz-Riley split at one frequency: a low-pass and a high-pass of
/// 24 dB per octave (two Butterworth sections each), which sum to an
/// all-pass.
#[derive(Clone, Copy, Default)]
pub(crate) struct Split {
    low: [Section; 2],
    high: [Section; 2],
}

impl Split {
    pub(crate) fn set(&mut self, hz: f64, rate: f64) {
        for section in &mut self.low {
            section.set(false, hz, rate);
        }
        for section in &mut self.high {
            section.set(true, hz, rate);
        }
    }

    /// The low and the high part of `x`.
    pub(crate) fn process(&mut self, x: f64) -> (f64, f64) {
        let low = self.low[0].process(x);
        let high = self.high[0].process(x);
        (self.low[1].process(low), self.high[1].process(high))
    }
}

/// One channel's three-band split. All four splits run on every sample, so
/// switching High or Low finds their states current.
#[derive(Clone, Copy, Default)]
pub(crate) struct Crossover {
    /// The lower crossover on the input.
    lower: Split,
    /// The upper crossover on the input (High on, Low off).
    upper: Split,
    /// The upper crossover on what is above the lower one (both on).
    upper_of_high: Split,
    /// The upper crossover on what is below the lower one: it brings the
    /// low band to the other two's phase, so the three sum to an all-pass.
    upper_of_low: Split,
}

impl Crossover {
    pub(crate) fn set(&mut self, lower: f64, upper: f64, rate: f64) {
        self.lower.set(lower, rate);
        for split in [
            &mut self.upper,
            &mut self.upper_of_high,
            &mut self.upper_of_low,
        ] {
            split.set(upper, rate);
        }
    }

    /// The low, mid and high bands of `x`; a band that doesn't exist is 0.
    pub(crate) fn process(&mut self, x: f64, low_on: bool, high_on: bool) -> [f64; BANDS] {
        let (below, above) = self.lower.process(x);
        let (upper_low, upper_high) = self.upper.process(x);
        let (mid, high) = self.upper_of_high.process(above);
        let (aligned_low, aligned_high) = self.upper_of_low.process(below);
        match (low_on, high_on) {
            (true, true) => [aligned_low + aligned_high, mid, high],
            (true, false) => [below, above, 0.0],
            (false, true) => [0.0, upper_low, upper_high],
            (false, false) => [0.0, x, 0.0],
        }
    }
}

/// The per-sample step of a one-pole whose time constant is `ms`
/// milliseconds: after `ms` it has closed the gap by 1 - 1/e.
fn step_of(ms: f64, rate: f64) -> f64 {
    let samples = (ms * 0.001 * rate).max(1.0);
    1.0 - (-1.0 / samples).exp()
}

/// The detector's level in dB for `level`, floored at [`FLOOR_DB`].
fn level_db(level: f64) -> f64 {
    20.0 * level.max(db_to_gain(FLOOR_DB)).log10()
}

/// One side's ratio, scaled by `amount` (a fraction) toward 1: the dB the
/// side's gain moves per dB the level is past its threshold, so 0 for a
/// ratio of 1. Above, a ratio over 1 cuts (negative); below, it boosts.
fn per_db(ratio: f64, amount: f64, above: bool) -> f64 {
    let slope = 1.0 / ratio.powf(amount) - 1.0;
    if above { slope } else { -slope }
}

/// One side's gain in dB for a detector level of `db`: the static curve.
/// `distance` is how far past the threshold the level is, over the knee;
/// a soft knee (`knee` over 0) spreads the turn over `knee` dB about the
/// threshold.
fn side_db(db: f64, threshold: f64, per_db: f64, knee: f64, above: bool) -> f64 {
    let over = if above {
        db - threshold
    } else {
        threshold - db
    };
    let half = 0.5 * knee;
    let distance = if knee > 0.0 && over.abs() < half {
        (over + half) * (over + half) / (2.0 * knee)
    } else {
        over.max(0.0)
    };
    (per_db * distance).clamp(MIN_GAIN_DB, MAX_BOOST_DB)
}

/// One side of a band, worked out for a stretch.
#[derive(Clone, Copy, Default)]
struct Side {
    threshold: f64,
    per_db: f64,
    attack: f64,
    release: f64,
}

/// A band, worked out for a stretch.
#[derive(Clone, Copy, Default)]
struct Plan {
    exists: bool,
    active: bool,
    solo: bool,
    above: Side,
    below: Side,
}

/// Multiband Dynamics's audio-thread half.
pub struct Triband {
    rate: f64,
    /// The crossovers' frequencies now, so they're redesigned only when
    /// they move.
    crossovers: (f64, f64),
    /// Each channel's signal, in place, and its crossover.
    buffer: [Vec<f32>; 2],
    cross: [Crossover; 2],
    /// The same for the sidechain key.
    key_cross: [Crossover; 2],
    /// Each band's and channel's mean square, for the RMS detector.
    mean_square: [[f64; 2]; BANDS],
    /// Each band's two sides' gains now, in dB: Above, then Below.
    gain_db: [[f64; 2]; BANDS],
    /// The loudest band input and output samples since the display last
    /// published, as linear amplitude.
    peak_in: [f32; BANDS],
    peak_out: [f32; BANDS],
}

impl Triband {
    fn new(config: &NodeConfig) -> Self {
        let frames = config.max_frames.max(1);
        Self {
            rate: config.sample_rate.as_f64(),
            crossovers: (0.0, 0.0),
            buffer: [vec![0.0; frames], vec![0.0; frames]],
            cross: [Crossover::default(); 2],
            key_cross: [Crossover::default(); 2],
            mean_square: [[0.0; 2]; BANDS],
            gain_db: [[0.0; 2]; BANDS],
            peak_in: [0.0; BANDS],
            peak_out: [0.0; BANDS],
        }
    }

    /// Redesigns the crossovers when a frequency moved.
    fn set_crossovers(&mut self, lower: f64, upper: f64) {
        if (lower, upper) == self.crossovers {
            return;
        }
        self.crossovers = (lower, upper);
        for cross in self.cross.iter_mut().chain(&mut self.key_cross) {
            cross.set(lower, upper, self.rate);
        }
    }
}

impl NativeDevice for Triband {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        // Where each parameter sits in the bank, which the description
        // always holds.
        let at = |id| params.index(id).expect("the description holds it");
        let value = |id| params.value(at(id));
        let on = |id| value(id) >= 0.5;

        let (low_on, high_on) = (on(LOW_ON), on(HIGH_ON));
        let lower = value(LOW_FREQ);
        self.set_crossovers(lower, value(HIGH_FREQ).max(lower));
        let knee = if on(SOFT_KNEE) { KNEE_DB } else { 0.0 };
        let rms = value(DETECTOR) as usize == DETECTOR_RMS;
        let rms_step = step_of(RMS_MS, self.rate);
        let time = value(TIME) * 0.01;
        let amount = value(AMOUNT) * 0.01;

        let mut plans = [Plan::default(); BANDS];
        for (band, plan) in plans.iter_mut().enumerate() {
            let param = |field| value(band_param(band, field));
            let side = |first: u32, above: bool| Side {
                threshold: param(first),
                per_db: per_db(param(first + 1), amount, above),
                attack: step_of(param(first + 2) * time, self.rate),
                release: step_of(param(first + 3) * time, self.rate),
            };
            let mut below = side(BELOW_THRESHOLD, false);
            let above = side(ABOVE_THRESHOLD, true);
            // The Below threshold never rises over the Above one.
            below.threshold = below.threshold.min(above.threshold);
            *plan = Plan {
                exists: match band {
                    LOW => low_on,
                    HIGH => high_on,
                    _ => true,
                },
                active: param(ACTIVE) >= 0.5,
                solo: param(SOLO) >= 0.5,
                above,
                below,
            };
        }
        let any_solo = plans.iter().any(|plan| plan.exists && plan.solo);

        let input = |band| params.ramp(at(band_param(band, INPUT)));
        let output = |band| params.ramp(at(band_param(band, BAND_OUTPUT)));
        let (input, output) = (
            [input(LOW), input(MID), input(HIGH)],
            [output(LOW), output(MID), output(HIGH)],
        );
        let master = params.ramp(at(OUTPUT));

        for channel in 0..channels {
            self.buffer[channel][..frames].copy_from_slice(audio.channel_mut(channel));
        }
        let keys = [
            audio.sidechain(0),
            if channels > 1 {
                audio.sidechain(1)
            } else {
                None
            },
        ];
        let keyed = keys[0].is_some();

        for i in 0..frames {
            let mut bands = [[0.0f64; 2]; BANDS];
            let mut key_bands = [[0.0f64; 2]; BANDS];
            for channel in 0..channels {
                let x = f64::from(self.buffer[channel][i]);
                let split = self.cross[channel].process(x, low_on, high_on);
                let key = keys[channel]
                    .map(|key| self.key_cross[channel].process(f64::from(key[i]), low_on, high_on));
                for band in 0..BANDS {
                    bands[band][channel] = split[band];
                    key_bands[band][channel] = key.map_or(0.0, |key| key[band]);
                }
            }

            let mut sum = [0.0f64; 2];
            for (band, plan) in plans.iter().enumerate() {
                if !plan.exists {
                    self.gain_db[band] = [0.0; 2];
                    continue;
                }
                let signal = bands[band];
                let mut peak = 0.0f32;
                for sample in &signal[..channels] {
                    peak = peak.max(sample.abs() as f32);
                }
                self.peak_in[band] = self.peak_in[band].max(peak);

                let mut played = signal;
                if plan.active {
                    let gain_in = f64::from(input[band][i]);
                    // 1. The detector's level: the loudest channel's, from
                    // the key's band when there is one.
                    let mut level = 0.0f64;
                    for channel in 0..channels {
                        let heard = if keyed {
                            key_bands[band][channel]
                        } else {
                            signal[channel] * gain_in
                        };
                        let detected = if rms {
                            let ms = &mut self.mean_square[band][channel];
                            *ms += (heard * heard - *ms) * rms_step;
                            ms.sqrt()
                        } else {
                            heard.abs()
                        };
                        level = level.max(detected);
                    }
                    let db = level_db(level);
                    // 2. Each side's gain, moving to its target with
                    // Attack as it grows and Release as it lets go.
                    let mut total = 0.0;
                    for (state, (side, above)) in self.gain_db[band]
                        .iter_mut()
                        .zip([(plan.above, true), (plan.below, false)])
                    {
                        let target = side_db(db, side.threshold, side.per_db, knee, above);
                        let step = if target.abs() > state.abs() {
                            side.attack
                        } else {
                            side.release
                        };
                        *state += (target - *state) * step;
                        total += *state;
                    }
                    // 3. The band through its Input, the gain and its Output.
                    let gain = gain_in * db_to_gain(total) * f64::from(output[band][i]);
                    for sample in &mut played {
                        *sample *= gain;
                    }
                } else {
                    self.gain_db[band] = [0.0; 2];
                }
                let mut peak = 0.0f32;
                for sample in &played[..channels] {
                    peak = peak.max(sample.abs() as f32);
                }
                self.peak_out[band] = self.peak_out[band].max(peak);
                if !any_solo || plan.solo {
                    for (sum, played) in sum.iter_mut().zip(played) {
                        *sum += played;
                    }
                }
            }
            for (buffer, sum) in self.buffer.iter_mut().zip(sum).take(channels) {
                buffer[i] = (sum * f64::from(master[i])) as f32;
            }
        }
        for channel in 0..channels {
            audio
                .channel_mut(channel)
                .copy_from_slice(&self.buffer[channel][..frames]);
        }
    }

    /// The deepest reduction of the last block, in dB: the most any band
    /// took off (the mixer view spec, R40).
    fn gain_reduction(&self) -> f32 {
        self.gain_db
            .iter()
            .fold(0.0f32, |most, band| most.min((band[0] + band[1]) as f32))
    }

    fn display_len(&self) -> usize {
        DISPLAY_VALUES
    }

    fn display(&mut self, out: &mut [f32]) {
        let db = |level: f32| level_db(f64::from(level)) as f32;
        for band in 0..BANDS {
            out[3 * band] = (self.gain_db[band][0] + self.gain_db[band][1]) as f32;
            out[3 * band + 1] = db(self.peak_in[band]);
            out[3 * band + 2] = db(self.peak_out[band]);
        }
        self.peak_in = [0.0; BANDS];
        self.peak_out = [0.0; BANDS];
    }
}

/// What the node publishes: for each band, the gain now in dB (the two
/// sides' sum, negative for a cut), and the band's input and output peaks
/// since the last block in dB.
const DISPLAY_VALUES: usize = 3 * BANDS;

/// The curves' input levels, in dB, one point per dB.
const CURVE_FROM_DB: f32 = -70.0;
const CURVE_POINTS: usize = 71;

/// The device's display (the multiband-dynamics spec, R13): each band's
/// transfer curve and its two thresholds as markers on it, from the
/// parameters; with an engine publishing, each band's gain reduction and
/// input and output levels as readings. `data` is what the node published,
/// if an engine runs.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let (low_on, high_on) = (value(LOW_ON) >= 0.5, value(HIGH_ON) >= 0.5);
    let knee = if value(SOFT_KNEE) >= 0.5 {
        KNEE_DB
    } else {
        0.0
    };
    let amount = value(AMOUNT) * 0.01;
    let published = data.and_then(|d| <[f32; DISPLAY_VALUES]>::try_from(d).ok());
    let mut out = DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    };
    let reading = |name: String, level: f32| DisplayReading {
        name,
        value: level,
        unit: "dB".to_owned(),
        text: format!("{level:.1} dB"),
    };
    for (band, name) in BAND_NAMES.iter().enumerate() {
        let exists = match band {
            LOW => low_on,
            HIGH => high_on,
            _ => true,
        };
        if !exists {
            continue;
        }
        let param = |field| value(band_param(band, field));
        let (above, below) = (
            (
                param(ABOVE_THRESHOLD),
                per_db(param(ABOVE_RATIO), amount, true),
            ),
            (
                param(BELOW_THRESHOLD),
                per_db(param(BELOW_RATIO), amount, false),
            ),
        );
        let below_threshold = below.0.min(above.0);
        let through = |db: f64| {
            let gain = if param(ACTIVE) >= 0.5 {
                side_db(db, above.0, above.1, knee, true)
                    + side_db(db, below_threshold, below.1, knee, false)
            } else {
                0.0
            };
            db + gain
        };
        let curve = format!("{name} Transfer Curve");
        out.series.push(DisplaySeries {
            name: curve.clone(),
            unit: "dB".to_owned(),
            x_unit: "dB".to_owned(),
            x_range: [CURVE_FROM_DB, 0.0],
            log_x: false,
            y_range: [CURVE_FROM_DB, 0.0],
            detail: false,
            values: (0..CURVE_POINTS)
                .map(|i| through(f64::from(CURVE_FROM_DB) + i as f64) as f32)
                .collect(),
        });
        for (side, threshold) in [("Above", above.0), ("Below", below_threshold)] {
            out.markers.push(DisplayMarker {
                name: format!("{name} {side} Threshold"),
                series: curve.clone(),
                x: threshold as f32,
                y: through(threshold) as f32,
                text: format!("{name} {side} Threshold {threshold:.1} dB"),
            });
        }
        if let Some(published) = published {
            out.readings.extend([
                reading(format!("{name} Gain Reduction"), published[3 * band]),
                reading(format!("{name} Input"), published[3 * band + 1]),
                reading(format!("{name} Output"), published[3 * band + 2]),
            ]);
        }
    }
    out
}
