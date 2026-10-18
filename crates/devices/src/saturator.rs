//! Singe, the native saturator (WP-5.23; the saturator spec): a waveshaper
//! with eight curves, a Color section that tilts what the shaper hears and
//! undoes it afterwards, a post-clip stage, and an optional oversampled
//! path.
//!
//! The signal goes through an optional DC filter, the Color filters, the
//! Drive gain, the curve, the Color filters' inverse, the post-clip stage and
//! Output, and is mixed with the dry signal. Hi-Quality runs the curve at 8
//! times the rate and delays the dry signal as long as the oversampler
//! delays the wet one. Every curve's function is Beatlayer's own, written
//! in the spec; the manual names the curves and the controls but not their
//! ranges, defaults or formulas.

use core::f32::consts::{FRAC_PI_2, PI};

use bl_dsp::Oversampler;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::db_to_gain;
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker,
    DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::eq::{Type, coeffs};
use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};
use crate::sections::Biquad;

/// Singe's device type.
pub const SATURATOR: &str = "beatlayer.singe";

/// Curve: one of [`CURVES`].
pub const CURVE: ParamId = ParamId(0);
/// Drive: the gain before the curve, in dB.
pub const DRIVE: ParamId = ParamId(1);
/// Threshold: where Bass Shaper starts to clip, in dB.
pub const THRESHOLD: ParamId = ParamId(2);
/// Output: the level the post-clip stage holds the output under.
pub const OUTPUT: ParamId = ParamId(3);
/// Post Clip: Off, Soft or Hard.
pub const POST_CLIP: ParamId = ParamId(4);
/// Dry/Wet, per cent.
pub const DRY_WET: ParamId = ParamId(5);
/// Color: whether the Color filters act.
pub const COLOR: ParamId = ParamId(6);
/// Amt Lo: how much more (or less) the low frequencies saturate, per cent.
pub const AMT_LO: ParamId = ParamId(7);
/// Amt Hi: the same for the band around Frequency, per cent.
pub const AMT_HI: ParamId = ParamId(8);
/// Frequency: the second Color filter's centre, in Hz.
pub const FREQUENCY: ParamId = ParamId(9);
/// Width: the second Color filter's bandwidth, in octaves.
pub const WIDTH: ParamId = ParamId(10);
/// The Waveshaper's Drive, per cent.
pub const WS_DRIVE: ParamId = ParamId(11);
/// The Waveshaper's Curve, per cent.
pub const WS_CURVE: ParamId = ParamId(12);
/// The Waveshaper's Depth, per cent.
pub const WS_DEPTH: ParamId = ParamId(13);
/// The Waveshaper's Linear, per cent.
pub const WS_LINEAR: ParamId = ParamId(14);
/// The Waveshaper's Damp, per cent.
pub const WS_DAMP: ParamId = ParamId(15);
/// The Waveshaper's Period: ripples of the superimposed sine.
pub const WS_PERIOD: ParamId = ParamId(16);
/// Hi-Quality, a menu item.
pub const HI_QUALITY: ParamId = ParamId(17);
/// Pre-DC Filter, a menu item.
pub const PRE_DC: ParamId = ParamId(18);

/// The curves, in the chooser's order.
pub const CURVES: [&str; 8] = [
    "Analog Clip",
    "Soft Sine",
    "Bass Shaper",
    "Medium Curve",
    "Hard Curve",
    "Sinoid Fold",
    "Digital Clip",
    "Waveshaper",
];
const CURVE_SOFT_SINE: usize = 1;
const CURVE_BASS_SHAPER: usize = 2;
const CURVE_SINOID_FOLD: usize = 5;
const CURVE_DIGITAL_CLIP: usize = 6;

/// The knees of the three tanh curves: below them the curve is the
/// identity.
const ANALOG_KNEE: f32 = 0.5;
const MEDIUM_KNEE: f32 = 0.2;
const HARD_KNEE: f32 = 0.8;

/// The post-clip choices.
const POST_SOFT: usize = 1;
const POST_HARD: usize = 2;

/// The Color filters: Amt Lo's low shelf corner, and the dB a full amount
/// boosts or cuts by.
const SHELF_HZ: f64 = 200.0;
const COLOR_RANGE_DB: f64 = 12.0;
/// The Pre-DC Filter's corner, in Hz.
const DC_HZ: f64 = 5.0;
/// Frames between coefficient updates of the Color filters. Counted from the
/// device's first frame, so a render doesn't depend on its block size.
const REFRESH_FRAMES: u64 = 16;
/// Hi-Quality: 8 times the rate.
const HI_QUALITY_STAGES: u32 = 3;

/// The curve display: points across inputs from `-CURVE_SPAN` to
/// `CURVE_SPAN`.
pub const CURVE_POINTS: usize = 97;
const CURVE_SPAN: f32 = 2.0;
const CURVE_PEAK: f32 = 1.25;
const LEVEL_FLOOR_DB: f32 = -120.0;

/// Linear to `knee`, then easing to 1 as a tanh: the Analog Clip, Medium
/// Curve and Hard Curve shape.
pub(crate) fn soft_knee(x: f32, knee: f32) -> f32 {
    let magnitude = x.abs();
    let room = 1.0 - knee;
    if magnitude <= knee {
        x
    } else if room <= 1e-6 {
        x.signum()
    } else {
        x.signum() * (knee + room * ((magnitude - knee) / room).tanh())
    }
}

/// Linear to `knee`, then easing to 1 as `u / (1 + u)`, which leaves the
/// knee more slowly than a tanh does: Bass Shaper.
fn rational_knee(x: f32, knee: f32) -> f32 {
    let magnitude = x.abs();
    let room = 1.0 - knee;
    if magnitude <= knee {
        x
    } else if room <= 1e-6 {
        x.signum()
    } else {
        let over = (magnitude - knee) / room;
        x.signum() * (knee + room * over / (1.0 + over))
    }
}

/// Linear to 0.5, then a quarter sine to 1, then flat: Soft Sine.
fn soft_sine(x: f32) -> f32 {
    let magnitude = x.abs();
    if magnitude <= 0.5 {
        x
    } else {
        x.signum() * (0.5 + 0.5 * (2.0 * (magnitude - 0.5)).min(FRAC_PI_2).sin())
    }
}

/// Linear to 0.5, then a sine that goes on past 1 and folds back: Sinoid
/// Fold.
fn sinoid_fold(x: f32) -> f32 {
    let magnitude = x.abs();
    if magnitude <= 0.5 {
        x
    } else {
        x.signum() * (0.5 + 0.5 * (2.0 * (magnitude - 0.5)).sin())
    }
}

/// The Waveshaper's six controls, each 0 to 1 but Period.
#[derive(Clone, Copy)]
struct Waveshaper {
    drive: f32,
    curve: f32,
    depth: f32,
    linear: f32,
    damp: f32,
    period: f32,
}

impl Waveshaper {
    /// The Waveshaper's output: the straight line at Drive 0, and at Drive
    /// 1 the cubic soft clip (Curve) plus a sine (Depth, Period), pulled
    /// toward the line by Linear and gated near the centre by Damp. Past
    /// ±1 the shaped part holds its end value.
    fn at(self, x: f32) -> f32 {
        let u = x.clamp(-1.0, 1.0);
        let core = u + self.curve * (0.5 * u - 0.5 * u * u * u);
        let ripple = 0.25 * self.depth * (PI * self.period * u).sin();
        let shaped = (1.0 - self.linear) * (core + ripple) + self.linear * u;
        let gate = if self.damp > 0.0 {
            let t = (u.abs() / (0.25 * self.damp)).min(1.0);
            t * t * (3.0 - 2.0 * t)
        } else {
            1.0
        };
        (1.0 - self.drive) * x + self.drive * shaped * gate
    }
}

/// A curve and the controls it reads, at one moment.
#[derive(Clone, Copy)]
struct Shaper {
    curve: usize,
    /// Bass Shaper's knee, a linear amplitude.
    knee: f32,
    waveshaper: Waveshaper,
}

impl Shaper {
    /// The curve `curve` with the controls' values: `threshold_db` and the
    /// six Waveshaper values (per cent, Period as it is), in the order
    /// [`WS_DRIVE`] to [`WS_PERIOD`].
    fn new(curve: usize, threshold_db: f32, ws: [f32; 6]) -> Self {
        let percent = |v: f32| (v / 100.0).clamp(0.0, 1.0);
        Self {
            curve,
            knee: db_to_gain(f64::from(threshold_db)) as f32,
            waveshaper: Waveshaper {
                drive: percent(ws[0]),
                curve: percent(ws[1]),
                depth: percent(ws[2]),
                linear: percent(ws[3]),
                damp: percent(ws[4]),
                period: ws[5],
            },
        }
    }

    fn at(self, x: f32) -> f32 {
        match self.curve {
            0 => soft_knee(x, ANALOG_KNEE),
            CURVE_SOFT_SINE => soft_sine(x),
            CURVE_BASS_SHAPER => rational_knee(x, self.knee),
            3 => soft_knee(x, MEDIUM_KNEE),
            4 => soft_knee(x, HARD_KNEE),
            CURVE_SINOID_FOLD => sinoid_fold(x),
            CURVE_DIGITAL_CLIP => x.clamp(-1.0, 1.0),
            _ => self.waveshaper.at(x),
        }
    }
}

/// The Singe device type.
#[must_use]
pub fn saturator_type() -> DeviceType {
    let percent = |id, name: &str, default| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
    };
    let amount = |id, name: &str| {
        ParamDescriptor::continuous(id, name, (-100.0, 100.0), 0.0, ParamUnit::Percent)
    };
    let params = vec![
        ParamDescriptor::choice(CURVE, "Curve", &CURVES, 0)
            .describe("The curve that shapes each sample of the signal."),
        ParamDescriptor::continuous(DRIVE, "Drive", (-36.0, 36.0), 5.0, ParamUnit::Decibels)
            .describe("The gain applied before the curve."),
        ParamDescriptor::continuous(
            THRESHOLD,
            "Threshold",
            (-50.0, 0.0),
            -12.0,
            ParamUnit::Decibels,
        )
        .describe(
            "Bass Shaper only: where the curve starts to clip. Low values clip softly and \
             0 dB clips hard.",
        ),
        ParamDescriptor::gain(OUTPUT, "Output", 6.0, 0.0)
            .describe("The level of the shaped signal, and the level Post Clip holds it under."),
        ParamDescriptor::choice(POST_CLIP, "Post Clip", &["Off", "Soft", "Hard"], 0).describe(
            "A clipping stage after the curve and the Color filters, at the Output level: \
             Soft rounds the peaks and Hard cuts them.",
        ),
        percent(DRY_WET, "Dry/Wet", 100.0)
            .describe("The balance between the dry signal and the saturated one."),
        ParamDescriptor::toggle(COLOR, "Color", false).describe(
            "Switches on two filters that shape the signal before the curve, and their inverse \
             after it.",
        ),
        amount(AMT_LO, "Amt Lo")
            .describe("Makes the low frequencies saturate more (above 0) or less (below 0)."),
        amount(AMT_HI, "Amt Hi")
            .describe("Makes the band around Frequency saturate more (above 0) or less (below 0)."),
        ParamDescriptor::continuous(
            FREQUENCY,
            "Frequency",
            (50.0, 15_000.0),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The centre of the second Color filter."),
        ParamDescriptor::continuous(WIDTH, "Width", (0.1, 4.0), 1.0, ParamUnit::None)
            .describe("The width of the second Color filter, in octaves."),
        percent(WS_DRIVE, "Waveshaper Drive", 100.0)
            .describe("How far the Waveshaper's controls shape the curve; at 0% it stays straight."),
        percent(WS_CURVE, "Waveshaper Curve", 50.0)
            .describe("Bends the Waveshaper's curve, adding mostly third harmonics."),
        percent(WS_DEPTH, "Waveshaper Depth", 0.0)
            .describe("The size of the sine ripple added to the Waveshaper's curve."),
        percent(WS_LINEAR, "Waveshaper Linear", 0.0)
            .describe("Pulls the Waveshaper's curve, ripples included, toward a straight line."),
        percent(WS_DAMP, "Waveshaper Damp", 0.0)
            .describe("Flattens small signals around the centre, like a very fast noise gate."),
        ParamDescriptor::continuous(WS_PERIOD, "Waveshaper Period", (1.0, 16.0), 4.0, ParamUnit::None)
            .describe("How dense the ripples of the added sine are."),
        ParamDescriptor::toggle(HI_QUALITY, "Hi-Quality", false)
            .describe("Runs the curve at eight times the rate so its harmonics don't alias; it delays the output."),
        ParamDescriptor::toggle(PRE_DC, "Pre-DC Filter", false)
            .describe("Removes a DC offset from the input before the curve."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (CURVE, Widget::Chooser),
                (THRESHOLD, Widget::Knob),
                (POST_CLIP, Widget::Chooser),
            ],
        )),
        LayoutNode::Cell(
            DisplayCell::new("Shaper Curve", 150.0, BODY_HEIGHT, 1.0)
                .with_bottom(ControlBand::stacked(60.0, &[(DRIVE, Widget::Knob)])),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (OUTPUT, Widget::Knob),
                (DRY_WET, Widget::Knob),
                (COLOR, Widget::Toggle),
                (AMT_LO, Widget::Knob),
            ],
        )),
    ]))
    .with_expanded(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (AMT_HI, Widget::Knob),
                (FREQUENCY, Widget::Knob),
                (WIDTH, Widget::Knob),
            ],
        )),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (WS_DRIVE, Widget::Knob),
                (WS_CURVE, Widget::Knob),
                (WS_DEPTH, Widget::Knob),
                (WS_LINEAR, Widget::Knob),
                (WS_DAMP, Widget::Knob),
                (WS_PERIOD, Widget::Knob),
            ],
        )),
    ]));
    DeviceType::native(SATURATOR, "Singe", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_menu_options(&[HI_QUALITY, PRE_DC]),
    )
}

/// Adds Singe to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, saturator_type(), make);
}

/// Makes a Singe for `device`'s values and `config`'s rate, channels and
/// block size. Every buffer is made here, off the audio thread, and its
/// latency comes from the set's Hi-Quality value, so it is reported before
/// the device first runs.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> Singe {
    Singe::new(device, config)
}

/// One channel's filters: the DC filter's memory, and the Color filters
/// before and after the curve (a low shelf and a peak each).
#[derive(Clone, Copy, Default)]
struct Channel {
    dc_in: f32,
    dc_out: f32,
    before: [Biquad; 2],
    after: [Biquad; 2],
}

/// Sets `filters` to the Color filters at frame `i` of the stretch, with
/// their gains times `sign` (so -1 undoes +1).
fn set_color(filters: &mut [Biquad; 2], params: &ParamBank, i: usize, sign: f64, rate: f64) {
    let ramp = |id: ParamId| f64::from(params.ramp(id.0 as usize)[i]);
    let low = sign * COLOR_RANGE_DB * ramp(AMT_LO) / 100.0;
    let high = sign * COLOR_RANGE_DB * ramp(AMT_HI) / 100.0;
    // Bandwidth in octaves to the cookbook's Q.
    let q = 1.0 / (2.0 * (core::f64::consts::LN_2 / 2.0 * ramp(WIDTH)).sinh());
    filters[0].c = coeffs(Type::LowShelf, SHELF_HZ, low, 0.71, rate);
    filters[1].c = coeffs(Type::Peak, ramp(FREQUENCY), high, q, rate);
}

/// Singe's audio-thread half.
pub struct Singe {
    rate: f64,
    channels: [Channel; 2],
    /// The input as it came, for the dry signal.
    dry: [Vec<f32>; 2],
    /// The signal on its way through the curve.
    work: [Vec<f32>; 2],
    oversamplers: [Oversampler; 2],
    delay: [DelayLine; 2],
    /// Frames processed so far, which the Color filters' refresh counts.
    ticks: u64,
    hi_quality: bool,
    latency: u32,
    /// The input's peak since the last block, for the display.
    peak: f32,
}

impl Singe {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let frames = config.max_frames.max(1);
        let buffer = || vec![0.0f32; frames];
        let oversampler = || Oversampler::new(HI_QUALITY_STAGES, config.channels.max(1), frames);
        let hi_quality = device.params.get(&HI_QUALITY).copied().unwrap_or(0.0) >= 0.5;
        let oversamplers = [oversampler(), oversampler()];
        let latency = oversamplers[0].latency();
        let rate = config.sample_rate.as_f64();
        // Filters that do nothing until the first refresh sets them.
        let mut flat = Biquad::default();
        flat.c = coeffs(Type::Peak, 1_000.0, 0.0, 0.71, rate);
        let channel = Channel {
            before: [flat; 2],
            after: [flat; 2],
            ..Channel::default()
        };
        Self {
            rate,
            channels: [channel; 2],
            dry: [buffer(), buffer()],
            work: [buffer(), buffer()],
            delay: [
                DelayLine::new(latency as usize),
                DelayLine::new(latency as usize),
            ],
            oversamplers,
            ticks: 0,
            hi_quality,
            latency: if hi_quality { latency } else { 0 },
            peak: 0.0,
        }
    }
}

/// The curve at frame `i` of the stretch, from the parameters' ramps.
fn shaper_at(params: &ParamBank, curve: usize, i: usize) -> Shaper {
    let ramp = |id: ParamId| params.ramp(id.0 as usize)[i];
    Shaper::new(
        curve,
        ramp(THRESHOLD),
        [
            ramp(WS_DRIVE),
            ramp(WS_CURVE),
            ramp(WS_DEPTH),
            ramp(WS_LINEAR),
            ramp(WS_DAMP),
            ramp(WS_PERIOD),
        ],
    )
}

impl NativeDevice for Singe {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let value = |id: ParamId| params.value(id.0 as usize);
        let curve = (value(CURVE) as usize).min(CURVES.len() - 1);
        let post = (value(POST_CLIP) as usize).min(POST_HARD);
        let (color, dc) = (value(COLOR) >= 0.5, value(PRE_DC) >= 0.5);
        let hi_quality = value(HI_QUALITY) >= 0.5;
        if hi_quality && !self.hi_quality {
            for oversampler in &mut self.oversamplers {
                oversampler.reset();
            }
        }
        self.hi_quality = hi_quality;
        let latency = if hi_quality {
            self.oversamplers[0].latency()
        } else {
            0
        };
        self.latency = latency;
        let dc_pole = (1.0 - core::f64::consts::TAU * DC_HZ / self.rate) as f32;
        let (drive, output, mix) = (
            params.ramp(DRIVE.0 as usize),
            params.ramp(OUTPUT.0 as usize),
            params.ramp(DRY_WET.0 as usize),
        );

        // 1. The input to the curve: the DC filter, the Color filters, Drive.
        for channel in 0..channels {
            let state = &mut self.channels[channel];
            let input = audio.channel_mut(channel);
            self.dry[channel][..frames].copy_from_slice(input);
            let work = &mut self.work[channel][..frames];
            for (i, sample) in input.iter().enumerate() {
                self.peak = self.peak.max(sample.abs());
                let mut v = *sample;
                if dc {
                    let filtered = v - state.dc_in + dc_pole * state.dc_out;
                    state.dc_in = v;
                    state.dc_out = filtered;
                    v = filtered;
                } else {
                    state.dc_in = v;
                    state.dc_out = 0.0;
                }
                if color {
                    if (self.ticks + i as u64).is_multiple_of(REFRESH_FRAMES) {
                        set_color(&mut state.before, params, i, 1.0, self.rate);
                    }
                    v = state.before[0].process(v);
                    v = state.before[1].process(v);
                }
                work[i] = v * db_to_gain(f64::from(drive[i])) as f32;
            }
        }

        // 2. The curve, at the rate or at 8 times it.
        for channel in 0..channels {
            let work = &mut self.work[channel][..frames];
            if hi_quality {
                let factor = self.oversamplers[channel].factor();
                let mut frame = 0;
                self.oversamplers[channel].process(channel, work, |high| {
                    let mut shaper = shaper_at(params, curve, frame.min(frames - 1));
                    let mut at = frame;
                    for (k, sample) in high.iter_mut().enumerate() {
                        if frame + k / factor != at {
                            at = frame + k / factor;
                            shaper = shaper_at(params, curve, at.min(frames - 1));
                        }
                        *sample = shaper.at(*sample);
                    }
                    frame += high.len() / factor;
                });
            } else {
                for (i, sample) in work.iter_mut().enumerate() {
                    *sample = shaper_at(params, curve, i).at(*sample);
                }
            }
        }

        // 3. The Color filters' inverse, Post Clip, Output and the mix.
        for channel in 0..channels {
            let state = &mut self.channels[channel];
            let work = &self.work[channel][..frames];
            let dry = &self.dry[channel][..frames];
            let delay = &mut self.delay[channel];
            let out = audio.channel_mut(channel);
            for i in 0..frames {
                let mut v = work[i];
                if color {
                    if (self.ticks + i as u64).is_multiple_of(REFRESH_FRAMES) {
                        set_color(&mut state.after, params, i, -1.0, self.rate);
                    }
                    v = state.after[0].process(v);
                    v = state.after[1].process(v);
                }
                v = match post {
                    POST_SOFT => soft_knee(v, ANALOG_KNEE),
                    POST_HARD => v.clamp(-1.0, 1.0),
                    _ => v,
                };
                v *= output[i];
                let dry_now = if latency > 0 {
                    delay.read(latency as usize)
                } else {
                    dry[i]
                };
                delay.write(dry[i]);
                out[i] = dry_now + (v - dry_now) * (mix[i] / 100.0);
            }
        }
        self.ticks += frames as u64;
    }

    fn latency(&self) -> u32 {
        self.latency
    }

    fn display_len(&self) -> usize {
        1
    }

    fn display(&mut self, out: &mut [f32]) {
        out[0] = level_db(self.peak);
        self.peak = 0.0;
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

/// The device's display (the saturator spec, R14): the curve over inputs
/// from -2 to 2, and, once an engine publishes, the input's level now as a
/// reading and a marker on it. `data` is what the node published.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0) as f32;
    let curve = (value(CURVE) as usize).min(CURVES.len() - 1);
    let shaper = Shaper::new(
        curve,
        value(THRESHOLD),
        [
            value(WS_DRIVE),
            value(WS_CURVE),
            value(WS_DEPTH),
            value(WS_LINEAR),
            value(WS_DAMP),
            value(WS_PERIOD),
        ],
    );
    let series = DisplaySeries {
        name: "Shaper Curve".to_owned(),
        unit: String::new(),
        x_unit: String::new(),
        x_range: [-CURVE_SPAN, CURVE_SPAN],
        log_x: false,
        y_range: [-CURVE_PEAK, CURVE_PEAK],
        detail: false,
        values: (0..CURVE_POINTS)
            .map(|i| {
                shaper.at(-CURVE_SPAN + 2.0 * CURVE_SPAN * i as f32 / (CURVE_POINTS - 1) as f32)
            })
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
    if let Some([input]) = data.and_then(|d| <[f32; 1]>::try_from(d).ok()) {
        out.readings.push(DisplayReading {
            name: "Input".to_owned(),
            value: input,
            unit: "dB".to_owned(),
            text: format!("{input:.1} dB"),
        });
        if input > LEVEL_FLOOR_DB {
            let x = (db_to_gain(f64::from(input + value(DRIVE))) as f32).min(CURVE_SPAN);
            out.markers.push(DisplayMarker {
                name: "Level".to_owned(),
                series: "Shaper Curve".to_owned(),
                x,
                y: shaper.at(x),
                text: format!("Level {input:.1} dB"),
            });
        }
    }
    out
}
