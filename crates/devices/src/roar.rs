//! Roar, the native Roar (WP-8.38; the roar spec, part 1): an input section
//! (Drive and a Tone tilt that Color Compensation undoes afterwards), three
//! gain stages each with a shaper and a filter, seven routing modes, a
//! 4 times oversampled path, a global output and the shaper's display.
//!
//! The signal goes through Drive and the Tone tilt, then, at four times the
//! rate, through the stages as the routing mode joins them, and back down
//! through the tilt's inverse, Output Gain, a hard clip and the mix with the
//! dry signal. Every curve and filter is Beatlayer's own, written in the
//! spec; the manual names them but gives no ranges or formulas. The six
//! filters Roar shares with the Auto Filter are its filters ([`crate::filters`]).

use core::f64::consts::{FRAC_PI_2, PI};

use bl_dsp::{Oversampler, Random};
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::db_to_gain;
use bl_model::{
    Bank, Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole,
    DisplayMarker, DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::filters::{Chan, Circuit, Coef, Knobs, Plan, Stage, flush, q_of};
use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};
use crate::multiband_dynamics::Split;

/// Roar's device type.
pub const ROAR: &str = "beatlayer.roar";

/// Drive: the gain into every stage, in dB.
pub const DRIVE: ParamId = ParamId(0);
/// Tone Amount: the tilt's depth, per cent.
pub const TONE_AMOUNT: ParamId = ParamId(1);
/// Tone Frequency: the tilt's centre, in Hz.
pub const TONE_FREQUENCY: ParamId = ParamId(2);
/// Color Compensation: undoes the tilt after the stages.
pub const COLOR_COMPENSATION: ParamId = ParamId(3);
/// Routing Mode: one of [`MODES`].
pub const ROUTING_MODE: ParamId = ParamId(4);
/// Blend: the crossfade between a mode's two signals, per cent.
pub const BLEND: ParamId = ParamId(5);
/// Low Crossover: Multi Band's lower split, in Hz.
pub const LOW_CROSSOVER: ParamId = ParamId(6);
/// High Crossover: Multi Band's upper split, in Hz.
pub const HIGH_CROSSOVER: ParamId = ParamId(7);
/// Output Gain: the wet signal's level, before the hard clip.
pub const OUTPUT_GAIN: ParamId = ParamId(8);
/// Dry/Wet, per cent.
pub const DRY_WET: ParamId = ParamId(9);

/// A stage's switch, within its block of parameters (see [`stage_param`]).
pub const ON: u32 = 0;
/// Shaper: one of [`SHAPERS`].
pub const SHAPER: u32 = 1;
/// Shaper Amount, per cent.
pub const AMOUNT: u32 = 2;
/// Shaper Bias, per cent.
pub const BIAS: u32 = 3;
/// Shaper Level: the stage's output gain.
pub const LEVEL: u32 = 4;
/// Filter: one of [`FILTERS`].
pub const FILTER: u32 = 5;
/// The filter's frequency, in Hz.
pub const FREQUENCY: u32 = 6;
/// The filter's Resonance, per cent.
pub const RESONANCE: u32 = 7;
/// Peak Gain, in dB.
pub const PEAK_GAIN: u32 = 8;
/// Morph, per cent.
pub const MORPH: u32 = 9;
/// Pre: the filter before the shaper.
pub const PRE: u32 = 10;

const FIRST_STAGE: u32 = 10;
const PER_STAGE: u32 = 11;

/// The stages in a device.
pub const STAGES: usize = 3;

/// The parameter `offset` ([`ON`] to [`PRE`]) of stage `stage` (0 to 2).
#[must_use]
pub const fn stage_param(stage: usize, offset: u32) -> ParamId {
    ParamId(FIRST_STAGE + PER_STAGE * stage as u32 + offset)
}

/// The routing modes, in the chooser's order.
pub const MODES: [&str; 7] = [
    "Single",
    "Serial",
    "Parallel",
    "Multi Band",
    "Mid Side",
    "Feedback",
    "Delay",
];
const SERIAL: usize = 1;
const PARALLEL: usize = 2;
const MULTI_BAND: usize = 3;
const MID_SIDE: usize = 4;
const FEEDBACK: usize = 5;
const DELAY: usize = 6;

/// The shaper curves, in the chooser's order.
pub const SHAPERS: [&str; 12] = [
    "Soft Sine",
    "Digital Clip",
    "Bit Crusher",
    "Diode Clipper",
    "Tube Preamp",
    "Half Wave Rectifier",
    "Full Wave Rectifier",
    "Polynomial",
    "Fractal",
    "Tri Fold",
    "Noise Injection",
    "Shards",
];
const BIT_CRUSHER: usize = 2;
const NOISE_INJECTION: usize = 10;

/// The filter types, in the chooser's order.
pub const FILTERS: [&str; 9] = [
    "LP",
    "BP",
    "HP",
    "Notch",
    "Peak",
    "Morph",
    "Comb",
    "Resampling",
    "Dispersion",
];
const PEAK: usize = 4;
const MORPH_FILTER: usize = 5;
const DISPERSION: usize = 8;

/// The Auto Filter's type for each of Roar's filters; Peak and Dispersion
/// are Roar's own.
const SHARED: [Option<usize>; 9] = [
    Some(0),
    Some(2),
    Some(1),
    Some(3),
    None,
    Some(5),
    Some(7),
    Some(8),
    None,
];

/// The display's views, one per stage.
const VIEWS: [&str; STAGES] = ["Stage 1", "Stage 2", "Stage 3"];

/// The stages run at four times the rate.
const OVERSAMPLE_STAGES: u32 = 2;
const FACTOR: usize = 1 << OVERSAMPLE_STAGES;

/// The shaper's gain at Amount 100 %, in dB.
const SHAPER_DB: f64 = 36.0;
/// The tilt's ends at Tone Amount 100 %, in dB.
const TONE_DB: f64 = 12.0;
/// The DC blocker's corner, in Hz.
const DC_HZ: f64 = 5.0;
/// A change of shaper or filter type crossfades over this long.
const FADE_SECONDS: f64 = 0.010;
/// Frames between the tilt's coefficient updates, and samples (at the high
/// rate) between the crossovers'. Counted from the device's first frame, so a
/// render doesn't depend on its block size.
const TONE_REFRESH: u64 = 16;
const CROSS_REFRESH: u64 = 64;
/// Dispersion's allpass sections.
const SECTIONS: usize = 8;
/// The Feedback and Delay modes' loop, until the feedback section sets them,
/// and what returns round it.
const LOOP_SECONDS: f64 = 0.1;
const LOOP_GAIN: f64 = 0.5;
/// The shaper display: points across inputs from -1 to 1.
pub const CURVE_POINTS: usize = 128;
const CURVE_PEAK: f32 = 2.0;
const LEVEL_FLOOR_DB: f32 = -120.0;

/// The shaper curve `kind` at `u` (the spec's R4): `amount` from 0 to 1 and
/// `noise`, a white noise value from -1 to 1.
fn curve(kind: usize, u: f64, amount: f64, noise: f64) -> f64 {
    let c = u.clamp(-1.0, 1.0);
    match kind {
        0 => (FRAC_PI_2 * c).sin(),
        1 => c,
        BIT_CRUSHER => {
            let steps = (15.0 - 14.0 * amount).exp2();
            (c * steps).round() / steps
        }
        3 => u / (1.0 + u.abs().powf(2.5)).powf(0.4),
        4 => {
            if u >= 0.0 {
                u.tanh()
            } else {
                0.7 * (u / 0.7).tanh()
            }
        }
        5 => u.max(0.0).tanh(),
        6 => u.abs().tanh(),
        7 => 1.5 * (c - c * c * c / 3.0),
        8 => {
            (0..4)
                .map(|k| 0.5f64.powi(k) * (f64::from(1 << k) * FRAC_PI_2 * c).sin())
                .sum::<f64>()
                / 1.875
        }
        9 => 2.0 / PI * (FRAC_PI_2 * u).sin().asin(),
        NOISE_INJECTION => (u + amount * noise * u.abs()).tanh(),
        _ => {
            let s = 8.0 * c;
            (s.floor() + (s - s.floor()).powi(4)) / 8.0
        }
    }
}

/// A shaper's output for the input `x`: the curve of `g x + bias` less the
/// curve at the bias, so silence stays silence. `amount` and `bias` are
/// fractions (0 to 1, -1 to 1).
fn shape(kind: usize, amount: f64, bias: f64, noise: f64, x: f64) -> f64 {
    let gain = if kind == BIT_CRUSHER {
        1.0
    } else {
        db_to_gain(amount * SHAPER_DB)
    };
    curve(kind, gain * x + bias, amount, noise) - curve(kind, bias, amount, noise)
}

/// Roar's device type.
#[must_use]
pub fn roar_type() -> DeviceType {
    let percent = |id, name: &str, default| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
    };
    let mut params = vec![
        ParamDescriptor::continuous(DRIVE, "Drive", (0.0, 36.0), 0.0, ParamUnit::Decibels)
            .describe("The gain into every gain stage."),
        ParamDescriptor::continuous(
            TONE_AMOUNT,
            "Tone Amount",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Tilts the spectrum about Tone Frequency before the stages: highs up above 0, lows up below."),
        ParamDescriptor::continuous(
            TONE_FREQUENCY,
            "Tone Frequency",
            (100.0, 10_000.0),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The centre of the Tone tilt."),
        ParamDescriptor::toggle(COLOR_COMPENSATION, "Color Compensation", false)
            .describe("Applies the Tone tilt the other way after the stages, so only the stages hear it."),
        ParamDescriptor::choice(ROUTING_MODE, "Routing Mode", &MODES, 0)
            .describe("How the gain stages are joined."),
        percent(BLEND, "Blend", 50.0).describe(
            "Crossfades between the routing mode's two signals; Single and Multi Band have none.",
        ),
        ParamDescriptor::continuous(
            LOW_CROSSOVER,
            "Low Crossover",
            (50.0, 2_000.0),
            250.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("Multi Band: where the low band ends."),
        ParamDescriptor::continuous(
            HIGH_CROSSOVER,
            "High Crossover",
            (500.0, 16_000.0),
            2_500.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("Multi Band: where the high band begins."),
        ParamDescriptor::gain(OUTPUT_GAIN, "Output Gain", 12.0, 0.0)
            .describe("The level of the wet signal, which then passes a hard clip at 0 dBFS."),
        percent(DRY_WET, "Dry/Wet", 100.0)
            .describe("The balance between the dry signal and the processed one."),
    ];
    for stage in 0..STAGES {
        let n = stage + 1;
        let id = |offset| stage_param(stage, offset);
        let name = |what: &str| format!("Stage {n} {what}");
        params.extend([
            ParamDescriptor::toggle(id(ON), &format!("Stage {n}"), true)
                .describe("Switches the stage on; off, it passes the signal untouched."),
            ParamDescriptor::choice(id(SHAPER), &name("Shaper"), &SHAPERS, 0)
                .describe("The curve that shapes the signal."),
            percent(id(AMOUNT), &name("Amount"), 0.0)
                .describe("How hard the shaper drives its curve."),
            ParamDescriptor::continuous(
                id(BIAS),
                &name("Bias"),
                (-100.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe("Shifts the signal on the curve, making its distortion lopsided."),
            ParamDescriptor::gain(id(LEVEL), &name("Level"), 12.0, 0.0)
                .describe("The level of the stage's output."),
            ParamDescriptor::choice(id(FILTER), &name("Filter"), &FILTERS, 0)
                .describe("The stage's filter."),
            ParamDescriptor::continuous(
                id(FREQUENCY),
                &name("Frequency"),
                (20.0, 20_000.0),
                20_000.0,
                ParamUnit::Hertz,
            )
            .logarithmic()
            .describe("The filter's frequency."),
            percent(id(RESONANCE), &name("Resonance"), 0.0).describe(
                "The filter's resonance; Comb's feedback and Dispersion's Q. Resampling has none.",
            ),
            ParamDescriptor::continuous(
                id(PEAK_GAIN),
                &name("Peak Gain"),
                (-24.0, 24.0),
                0.0,
                ParamUnit::Decibels,
            )
            .describe("Peak only: the gain at the filter's frequency."),
            percent(id(MORPH), &name("Morph"), 0.0)
                .describe("Morph only: low-pass at 0, band-pass at 50 %, high-pass at 100 %."),
            ParamDescriptor::toggle(id(PRE), &name("Pre"), false)
                .describe("Puts the filter before the shaper instead of after it."),
        ]);
    }
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    let stage_band = |stage: usize| {
        use Widget::{Chooser, Knob, Toggle};
        let id = |offset| stage_param(stage, offset);
        band(&[
            (id(ON), Toggle),
            (id(SHAPER), Chooser),
            (id(AMOUNT), Knob),
            (id(BIAS), Knob),
            (id(LEVEL), Knob),
            (id(FILTER), Chooser),
            (id(FREQUENCY), Knob),
            (id(RESONANCE), Knob),
            (id(PEAK_GAIN), Knob),
            (id(MORPH), Knob),
            (id(PRE), Toggle),
        ])
    };
    let layout = {
        use Widget::{Chooser, Knob, Toggle};
        DeviceLayout::new(LayoutNode::Row(vec![
            band(&[
                (DRIVE, Knob),
                (TONE_AMOUNT, Knob),
                (TONE_FREQUENCY, Knob),
                (COLOR_COMPENSATION, Toggle),
            ]),
            LayoutNode::Cell(DisplayCell::new("Shaper", 150.0, BODY_HEIGHT, 1.0)),
            stage_band(0),
            stage_band(1),
            stage_band(2),
            band(&[
                (ROUTING_MODE, Chooser),
                (BLEND, Knob),
                (LOW_CROSSOVER, Knob),
                (HIGH_CROSSOVER, Knob),
                (OUTPUT_GAIN, Knob),
                (DRY_WET, Knob),
            ]),
        ]))
    };
    let shaper_drag: Vec<ParamId> = (0..STAGES).map(|s| stage_param(s, AMOUNT)).collect();
    let stage_ids = |stage: usize, offsets: &[u32]| -> Vec<ParamId> {
        offsets.iter().map(|o| stage_param(stage, *o)).collect()
    };
    let mut banks = vec![
        (
            "Main".to_owned(),
            vec![
                DRIVE,
                TONE_AMOUNT,
                ROUTING_MODE,
                BLEND,
                stage_param(0, AMOUNT),
                stage_param(0, FREQUENCY),
                OUTPUT_GAIN,
                DRY_WET,
            ],
        ),
        (
            "Input".to_owned(),
            vec![DRIVE, TONE_AMOUNT, TONE_FREQUENCY, COLOR_COMPENSATION],
        ),
    ];
    for stage in 0..STAGES {
        let n = stage + 1;
        banks.push((
            format!("Stage {n}"),
            stage_ids(stage, &[ON, SHAPER, AMOUNT, BIAS, LEVEL]),
        ));
        banks.push((
            format!("Stage {n} Filter"),
            stage_ids(
                stage,
                &[FILTER, FREQUENCY, RESONANCE, PEAK_GAIN, MORPH, PRE],
            ),
        ));
    }
    banks.push((
        "Global".to_owned(),
        vec![
            ROUTING_MODE,
            BLEND,
            LOW_CROSSOVER,
            HIGH_CROSSOVER,
            OUTPUT_GAIN,
            DRY_WET,
        ],
    ));
    let mut description = DeviceDescription::new(params)
        .with_layout(layout)
        .with_views(&VIEWS)
        .with_display_drag(&shaper_drag);
    description.banks = banks
        .into_iter()
        .map(|(name, params)| Bank { name, params })
        .collect();
    DeviceType::native(ROAR, "Roar", DeviceRole::AudioEffect).with_description(description)
}

/// Adds Roar to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, roar_type(), make);
}

/// Makes a Roar for `device`'s values and `config`'s rate, channels and block
/// size. Every buffer is made here, off the audio thread.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> Roar {
    Roar::new(device, config)
}

/// A first-order section `k (s + wz) / (s + wp)` by the bilinear transform,
/// its zero and pole prewarped to `zero_hz` and `pole_hz`. Swapping the two
/// and inverting `k` gives its exact inverse.
#[derive(Clone, Copy, Default)]
struct FirstOrder {
    b0: f64,
    b1: f64,
    a1: f64,
    x1: f64,
    y1: f64,
}

impl FirstOrder {
    fn set(&mut self, zero_hz: f64, pole_hz: f64, k: f64, rate: f64) {
        let warp = |hz: f64| (PI * hz.clamp(1.0, rate * 0.49) / rate).tan();
        let (wz, wp) = (warp(zero_hz), warp(pole_hz));
        self.b0 = k * (1.0 + wz) / (1.0 + wp);
        self.b1 = k * (wz - 1.0) / (1.0 + wp);
        self.a1 = (wp - 1.0) / (1.0 + wp);
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 - self.a1 * self.y1;
        self.x1 = x;
        self.y1 = flush(y);
        y
    }
}

/// The Tone tilt: a low shelf and a high shelf, first order, at the same
/// corner, that cut the lows and lift the highs by `db` at the ends of the
/// band (and the reverse for a negative `db`).
#[derive(Clone, Copy, Default)]
struct Tilt {
    low: FirstOrder,
    high: FirstOrder,
}

impl Tilt {
    fn set(&mut self, db: f64, hz: f64, rate: f64) {
        let gain = db_to_gain(db);
        let (zero, pole) = (hz / gain.sqrt(), hz * gain.sqrt());
        self.low.set(zero, pole, 1.0, rate);
        self.high.set(zero, pole, gain, rate);
    }

    fn process(&mut self, x: f64) -> f64 {
        self.high.process(self.low.process(x))
    }
}

/// One stage's filter, with the states of the types Roar adds to the shared
/// ones.
struct Filter {
    chan: Chan,
    section: Stage,
    dispersion: [Stage; SECTIONS],
}

impl Filter {
    fn new(rate: f64) -> Self {
        Self {
            // The shared comb's rings are sized for twice the rate it is
            // given, and `rate` is the high one.
            chan: Chan::new(rate / 2.0),
            section: Stage::default(),
            dispersion: [Stage::default(); SECTIONS],
        }
    }

    fn reset(&mut self) {
        self.chan.reset();
        self.section = Stage::default();
        self.dispersion = [Stage::default(); SECTIONS];
    }

    /// One sample through filter `kind` as stage plan `p` sets it, at `rate`.
    fn tick(&mut self, kind: usize, x: f64, p: &StagePlan, rate: f64) -> f64 {
        let g = (PI * p.hz.clamp(1.0, rate * 0.49) / rate).tan();
        match kind {
            PEAK => {
                let c = Coef::new(g, q_of(p.res));
                let [_, band, _] = self.section.tick(x, &c, Circuit::Svf);
                x + (p.peak - 1.0) * c.k * band
            }
            DISPERSION => {
                let c = Coef::new(g, 0.5 + 7.5 * p.res);
                let mut y = x;
                for section in &mut self.dispersion {
                    let [_, band, _] = section.tick(y, &c, Circuit::Svf);
                    y -= 2.0 * c.k * band;
                }
                y
            }
            _ => {
                let plan = Plan::new(
                    SHARED[kind].unwrap_or(0),
                    1,
                    0,
                    Knobs {
                        hz: p.hz,
                        res: p.res,
                        morph: if kind == MORPH_FILTER { p.morph } else { 0.0 },
                        control: 0.0,
                        pitch: 0.0,
                        formant: 0.0,
                    },
                );
                self.chan.tick(x, &plan, rate)
            }
        }
    }
}

/// A stage's settings at one frame.
#[derive(Clone, Copy)]
struct StagePlan {
    on: bool,
    pre: bool,
    shaper: usize,
    filter: usize,
    amount: f64,
    bias: f64,
    level: f64,
    hz: f64,
    res: f64,
    morph: f64,
    /// Peak Gain as a factor.
    peak: f64,
}

/// What the routing reads at one frame.
#[derive(Clone, Copy)]
struct Frame {
    stages: [StagePlan; STAGES],
    low: f64,
    high: f64,
    /// Blend's equal-power gains for the first and the second signal.
    blend: (f64, f64),
}

/// The settings at frame `i` of the stretch, from the parameters' ramps.
fn frame_at(params: &ParamBank, i: usize) -> Frame {
    let at = |id: ParamId| f64::from(params.ramp(id.0 as usize)[i]);
    let stages = core::array::from_fn(|s| {
        let at = |offset| at(stage_param(s, offset));
        StagePlan {
            on: at(ON) >= 0.5,
            pre: at(PRE) >= 0.5,
            shaper: (at(SHAPER) as usize).min(SHAPERS.len() - 1),
            filter: (at(FILTER) as usize).min(FILTERS.len() - 1),
            amount: at(AMOUNT) / 100.0,
            bias: at(BIAS) / 100.0,
            level: at(LEVEL),
            hz: at(FREQUENCY),
            res: at(RESONANCE) / 100.0,
            morph: at(MORPH) / 100.0,
            peak: db_to_gain(at(PEAK_GAIN)),
        }
    });
    let angle = at(BLEND) / 100.0 * FRAC_PI_2;
    Frame {
        stages,
        low: at(LOW_CROSSOVER),
        high: at(HIGH_CROSSOVER),
        blend: (angle.cos(), angle.sin()),
    }
}

/// What a stretch's loop needs besides the frames: the high rate, the
/// crossfade's length in samples, the DC blocker's pole and the loop's
/// length in samples.
#[derive(Clone, Copy)]
struct Setup {
    rate: f64,
    fade: f64,
    pole: f64,
    loop_len: usize,
}

/// One stage on one channel: its filters, the types it is crossfading from,
/// the DC blocker and the noise.
struct Voice {
    random: Random,
    filters: [Filter; 2],
    /// The filter slot playing the filter type now.
    active: usize,
    filter: Option<usize>,
    filter_from: usize,
    filter_left: f64,
    shaper: usize,
    shaper_from: usize,
    shaper_left: f64,
    dc_in: f64,
    dc_out: f64,
    /// The largest level that has reached the shaper since the display read.
    peak: f32,
}

impl Voice {
    fn new(seed: u64, rate: f64) -> Self {
        Self {
            random: Random::seeded(seed),
            filters: [Filter::new(rate), Filter::new(rate)],
            active: 0,
            filter: None,
            filter_from: 0,
            filter_left: 0.0,
            shaper: 0,
            shaper_from: 0,
            shaper_left: 0.0,
            dc_in: 0.0,
            dc_out: 0.0,
            peak: 0.0,
        }
    }

    /// Notes the types the stretch plays: a change starts a crossfade from
    /// the type before.
    fn sync(&mut self, plan: &StagePlan, fade: f64) {
        if self.filter.is_none() {
            self.shaper = plan.shaper;
            self.filter = Some(plan.filter);
            return;
        }
        if plan.shaper != self.shaper {
            self.shaper_from = self.shaper;
            self.shaper = plan.shaper;
            self.shaper_left = fade;
        }
        if self.filter != Some(plan.filter) {
            self.filter_from = self.filter.unwrap_or(0);
            self.filter = Some(plan.filter);
            self.active ^= 1;
            self.filters[self.active].reset();
            self.filter_left = fade;
        }
    }

    fn filter_step(&mut self, x: f64, p: &StagePlan, setup: &Setup) -> f64 {
        let kind = self.filter.unwrap_or(p.filter);
        let now = self.filters[self.active].tick(kind, x, p, setup.rate);
        if self.filter_left <= 0.0 {
            return now;
        }
        let from = self.filters[self.active ^ 1].tick(self.filter_from, x, p, setup.rate);
        let t = 1.0 - self.filter_left / setup.fade;
        self.filter_left -= 1.0;
        from + (now - from) * t
    }

    fn shape_step(&mut self, x: f64, p: &StagePlan, setup: &Setup) -> f64 {
        let noisy = self.shaper == NOISE_INJECTION
            || (self.shaper_left > 0.0 && self.shaper_from == NOISE_INJECTION);
        let noise = if noisy {
            self.random.next_bipolar()
        } else {
            0.0
        };
        let now = shape(self.shaper, p.amount, p.bias, noise, x);
        if self.shaper_left <= 0.0 {
            return now;
        }
        let from = shape(self.shaper_from, p.amount, p.bias, noise, x);
        let t = 1.0 - self.shaper_left / setup.fade;
        self.shaper_left -= 1.0;
        from + (now - from) * t
    }

    /// One sample through the stage; a stage that is off passes it as it is.
    fn run(&mut self, x: f64, p: &StagePlan, setup: &Setup) -> f64 {
        if !p.on {
            return x;
        }
        let mut y = x;
        if p.pre {
            y = self.filter_step(y, p, setup);
        }
        self.peak = self.peak.max(y.abs() as f32);
        y = self.shape_step(y, p, setup);
        if !p.pre {
            y = self.filter_step(y, p, setup);
        }
        let out = y - self.dc_in + setup.pole * self.dc_out;
        self.dc_in = y;
        self.dc_out = flush(out);
        out * p.level
    }
}

/// One channel's stages and routing state, at the high rate.
struct Net {
    voices: [Voice; STAGES],
    /// Multi Band's crossovers: the lower on the input, the upper on what is
    /// above it, and the upper on what is below it, which brings the low band
    /// to the other two's phase.
    splits: [Split; 3],
    line: DelayLine,
    /// Samples run so far, which the crossovers' refresh counts.
    ticks: u64,
}

impl Net {
    fn new(seed: u64, rate: f64, loop_len: usize) -> Self {
        Self {
            voices: core::array::from_fn(|s| Voice::new(seed.wrapping_add(s as u64), rate)),
            splits: [Split::default(); 3],
            line: DelayLine::new(loop_len),
            ticks: 0,
        }
    }

    /// Runs `block`, channel `channel`'s samples at the high rate, from frame
    /// `*frame` of the stretch, in routing mode `mode`.
    fn run(
        &mut self,
        channel: usize,
        mode: usize,
        block: &mut [f32],
        frame: &mut usize,
        params: &ParamBank,
        setup: &Setup,
    ) {
        let first = *frame;
        let frames = params.ramp(0).len();
        let mut at = frame_at(params, first.min(frames - 1));
        for (voice, plan) in self.voices.iter_mut().zip(&at.stages) {
            voice.sync(plan, setup.fade);
        }
        for (j, sample) in block.iter_mut().enumerate() {
            if j % FACTOR == 0 {
                at = frame_at(params, (first + j / FACTOR).min(frames - 1));
            }
            if self.ticks.is_multiple_of(CROSS_REFRESH) {
                self.splits[0].set(at.low, setup.rate);
                self.splits[1].set(at.high, setup.rate);
                self.splits[2].set(at.high, setup.rate);
            }
            self.ticks += 1;
            let x = if sample.is_finite() {
                f64::from(*sample)
            } else {
                0.0
            };
            let [p0, p1, p2] = &at.stages;
            let (first_gain, second_gain) = at.blend;
            let mix = |a: f64, b: f64| first_gain * a + second_gain * b;
            let [v0, v1, v2] = &mut self.voices;
            let y = match mode {
                SERIAL => {
                    let a = v0.run(x, p0, setup);
                    mix(a, v1.run(a, p1, setup))
                }
                PARALLEL => {
                    let a = v0.run(x, p0, setup);
                    mix(a, v1.run(x, p1, setup))
                }
                MULTI_BAND => {
                    let (below, above) = self.splits[0].process(x);
                    let (mid, high) = self.splits[1].process(above);
                    let (aligned_low, aligned_high) = self.splits[2].process(below);
                    v0.run(aligned_low + aligned_high, p0, setup)
                        + v1.run(mid, p1, setup)
                        + v2.run(high, p2, setup)
                }
                MID_SIDE if channel == 1 => v1.run(x, p1, setup),
                FEEDBACK => {
                    let a = v0.run(x, p0, setup);
                    let back = f64::from(self.line.read(setup.loop_len)) * LOOP_GAIN;
                    let y = mix(a, v1.run(back, p1, setup));
                    self.line.write(y as f32);
                    y
                }
                DELAY => {
                    let a = v0.run(x, p0, setup);
                    let heard = v1.run(f64::from(self.line.read(setup.loop_len)), p1, setup);
                    self.line.write((a + LOOP_GAIN * heard) as f32);
                    mix(a, heard)
                }
                _ => v0.run(x, p0, setup),
            };
            *sample = y as f32;
        }
        *frame += block.len() / FACTOR;
    }
}

/// Roar's audio-thread half.
pub struct Roar {
    rate: f64,
    nets: [Net; 2],
    pre: [Tilt; 2],
    post: [Tilt; 2],
    oversamplers: [Oversampler; 2],
    /// The input as it came, for the dry signal, and the signal on its way
    /// through the stages.
    dry: [Vec<f32>; 2],
    work: [Vec<f32>; 2],
    delay: [DelayLine; 2],
    latency: u32,
    loop_len: usize,
    /// Frames processed so far, which the tilts' refresh counts.
    ticks: u64,
}

impl Roar {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let frames = config.max_frames.max(1);
        let rate = config.sample_rate.as_f64();
        let high_rate = rate * FACTOR as f64;
        let oversampler = || Oversampler::new(OVERSAMPLE_STAGES, 2, frames);
        let oversamplers = [oversampler(), oversampler()];
        let latency = oversamplers[0].latency();
        let loop_len = FACTOR * (LOOP_SECONDS * rate).round() as usize;
        let seed = device.id.0.get().wrapping_mul(8);
        let buffer = || vec![0.0f32; frames];
        Self {
            rate,
            nets: [
                Net::new(seed, high_rate, loop_len),
                Net::new(seed.wrapping_add(4), high_rate, loop_len),
            ],
            pre: [Tilt::default(); 2],
            post: [Tilt::default(); 2],
            oversamplers,
            dry: [buffer(), buffer()],
            work: [buffer(), buffer()],
            delay: [
                DelayLine::new(latency as usize),
                DelayLine::new(latency as usize),
            ],
            latency,
            loop_len,
            ticks: 0,
        }
    }
}

impl NativeDevice for Roar {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let ramp = |id: ParamId| params.ramp(id.0 as usize);
        let value = |id: ParamId| params.value(id.0 as usize);
        let mode = (value(ROUTING_MODE) as usize).min(MODES.len() - 1);
        let compensate = value(COLOR_COMPENSATION) >= 0.5;
        let mid_side = mode == MID_SIDE && channels == 2;
        let (drive, tone, tone_hz) = (ramp(DRIVE), ramp(TONE_AMOUNT), ramp(TONE_FREQUENCY));
        let high_rate = self.rate * FACTOR as f64;
        let setup = Setup {
            rate: high_rate,
            fade: (FADE_SECONDS * high_rate).round(),
            pole: 1.0 - core::f64::consts::TAU * DC_HZ / high_rate,
            loop_len: self.loop_len,
        };

        // 1. Drive and the Tone tilt, and the mid and side.
        for channel in 0..channels {
            let input = audio.channel(channel);
            self.dry[channel][..frames].copy_from_slice(input);
            let tilt = &mut self.pre[channel];
            let work = &mut self.work[channel][..frames];
            for i in 0..frames {
                if (self.ticks + i as u64).is_multiple_of(TONE_REFRESH) {
                    let db = f64::from(tone[i]) / 100.0 * TONE_DB;
                    tilt.set(db, f64::from(tone_hz[i]), self.rate);
                }
                let x = f64::from(input[i]);
                let x = if x.is_finite() { x } else { 0.0 };
                work[i] = tilt.process(x * db_to_gain(f64::from(drive[i]))) as f32;
            }
        }
        if mid_side {
            let [left, right] = &mut self.work;
            for (l, r) in left[..frames].iter_mut().zip(&mut right[..frames]) {
                (*l, *r) = (0.5 * (*l + *r), 0.5 * (*l - *r));
            }
        }

        // 2. The stages, at four times the rate.
        for channel in 0..channels {
            let net = &mut self.nets[channel];
            let mut frame = 0;
            self.oversamplers[channel].process(
                channel,
                &mut self.work[channel][..frames],
                |high| {
                    net.run(channel, mode, high, &mut frame, params, &setup);
                },
            );
        }
        if mid_side {
            let [mid, side] = &mut self.work;
            for (m, s) in mid[..frames].iter_mut().zip(&mut side[..frames]) {
                (*m, *s) = (*m + *s, *m - *s);
            }
        }

        // 3. The tilt's inverse, Output Gain, the hard clip and the mix.
        let (output, mix) = (ramp(OUTPUT_GAIN), ramp(DRY_WET));
        for channel in 0..channels {
            let tilt = &mut self.post[channel];
            let work = &self.work[channel][..frames];
            let dry = &self.dry[channel][..frames];
            let delay = &mut self.delay[channel];
            let out = audio.channel_mut(channel);
            for i in 0..frames {
                if (self.ticks + i as u64).is_multiple_of(TONE_REFRESH) {
                    let db = if compensate {
                        -f64::from(tone[i]) / 100.0 * TONE_DB
                    } else {
                        0.0
                    };
                    tilt.set(db, f64::from(tone_hz[i]), self.rate);
                }
                let wet = tilt.process(f64::from(work[i])) as f32;
                let wet = (wet * output[i]).clamp(-1.0, 1.0);
                let dry_now = delay.read(self.latency as usize);
                delay.write(dry[i]);
                out[i] = dry_now + (wet - dry_now) * (mix[i] / 100.0);
            }
        }
        self.ticks += frames as u64;
    }

    fn latency(&self) -> u32 {
        self.latency
    }

    fn display_len(&self) -> usize {
        STAGES
    }

    fn display(&mut self, out: &mut [f32]) {
        for (stage, slot) in out.iter_mut().enumerate() {
            *slot = self
                .nets
                .iter_mut()
                .map(|net| core::mem::take(&mut net.voices[stage].peak))
                .fold(0.0, f32::max);
        }
    }
}

/// A level in dB, with a floor.
fn level_db(level: f32) -> f32 {
    if level > 1e-6 {
        20.0 * level.log10()
    } else {
        LEVEL_FLOOR_DB
    }
}

/// The device's display (the roar spec, R12) for the view `view` (a stage):
/// the stage's curve over inputs from -1 to 1 with its bias, and, once an
/// engine publishes, the level that reached the stage as a reading and a
/// marker on the curve. `data` is what the node published.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
    view: Option<&str>,
) -> DeviceDisplay {
    let stage = view
        .and_then(|view| VIEWS.iter().position(|name| *name == view))
        .unwrap_or(0);
    let value = |offset| {
        description
            .value(values, stage_param(stage, offset))
            .unwrap_or(0.0)
    };
    let (kind, amount, bias) = (
        (value(SHAPER) as usize).min(SHAPERS.len() - 1),
        value(AMOUNT) / 100.0,
        value(BIAS) / 100.0,
    );
    let at = |x: f64| shape(kind, amount, bias, 0.0, x) as f32;
    let series = DisplaySeries {
        name: "Shaper Curve".to_owned(),
        unit: String::new(),
        x_unit: String::new(),
        x_range: [-1.0, 1.0],
        log_x: false,
        y_range: [-CURVE_PEAK, CURVE_PEAK],
        detail: false,
        values: (0..CURVE_POINTS)
            .map(|i| at(-1.0 + 2.0 * i as f64 / (CURVE_POINTS - 1) as f64))
            .collect(),
    };
    let mut out = DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: vec![series],
        markers: Vec::new(),
        readings: Vec::new(),
    };
    if let Some(peak) = data.and_then(|d| d.get(stage)).copied() {
        let input = level_db(peak);
        out.readings.push(DisplayReading {
            name: "Input".to_owned(),
            value: input,
            unit: "dB".to_owned(),
            text: format!("{input:.1} dB"),
        });
        if input > LEVEL_FLOOR_DB {
            let x = peak.min(1.0);
            out.markers.push(DisplayMarker {
                name: "Level".to_owned(),
                series: "Shaper Curve".to_owned(),
                x,
                y: at(f64::from(x)),
                text: format!("Level {input:.1} dB"),
            });
        }
    }
    out
}
