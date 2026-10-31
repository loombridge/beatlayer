//! EQ Three, the native three-band EQ (WP-5.21; the three-band-eq spec): a
//! low, a mid and a high band, each with its gain (to silence, up to +6 dB)
//! and its On switch, split at FreqLo and FreqHi by crossovers of 24 or 48
//! dB per octave, and a signal light for each band.
//!
//! The crossovers are Linkwitz-Riley filters (RBJ sections in cascade). At
//! 24 dB (fourth order) the low band also passes an all-pass at FreqHi, so
//! the three bands sum flat at any settings. At 48 dB (eighth order) it
//! doesn't, so the sum dips where the crossovers sit close together: the
//! slight colouring the manual describes (see `docs/specs/three-band-eq.md`).

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayReading, OrdMap,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::eq::{Type, coeffs};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::sections::{Biquad, Coeffs, REFRESH_FRAMES};

/// EQ Three's device type.
pub const THREE_BAND_EQ: &str = "beatlayer.triad";

/// Gain Low, Gain Mid, Gain High: each band's gain, a linear factor from
/// silence to +6 dB.
pub const GAIN_LOW: ParamId = ParamId(0);
pub const GAIN_MID: ParamId = ParamId(1);
pub const GAIN_HIGH: ParamId = ParamId(2);
/// On Low, On Mid, On High: each band's switch.
pub const ON_LOW: ParamId = ParamId(3);
pub const ON_MID: ParamId = ParamId(4);
pub const ON_HIGH: ParamId = ParamId(5);
/// FreqLo: where the low band ends, in Hz.
pub const FREQ_LOW: ParamId = ParamId(6);
/// FreqHi: where the high band begins, in Hz.
pub const FREQ_HIGH: ParamId = ParamId(7);
/// Slope: 24 or 48 dB per octave.
pub const SLOPE: ParamId = ParamId(8);

/// The bands' top gain, in dB.
pub const TOP_DB: f64 = 6.0;
/// FreqLo's range and default, FreqHi's range and default, in Hz. The manual
/// gives only an example; these are Beatlayer's own.
pub const FREQ_LOW_RANGE: (f64, f64) = (20.0, 5_000.0);
pub const FREQ_LOW_DEFAULT: f64 = 250.0;
pub const FREQ_HIGH_RANGE: (f64, f64) = (200.0, 20_000.0);
pub const FREQ_HIGH_DEFAULT: f64 = 2_500.0;
/// The Slope chooser's names.
pub const SLOPES: [&str; 2] = ["24 dB", "48 dB"];
/// A band's signal light is lit while its level, a peak follower with a
/// release time constant of [`LIGHT_RELEASE_SECONDS`], is above this many dB.
pub const LIGHT_THRESHOLD_DB: f64 = -24.0;
const LIGHT_RELEASE_SECONDS: f64 = 0.1;

/// The most second-order sections one filter of a crossover has.
const MAX_STAGES: usize = 4;
/// A fourth-order Linkwitz-Riley filter is two Butterworth sections at Q
/// 1/sqrt(2); an eighth-order one is two fourth-order Butterworth filters,
/// whose sections' Qs are 1/(2 sin(pi/8)) and 1/(2 sin(3 pi/8)).
const Q_LR4: [f64; 2] = [std::f64::consts::FRAC_1_SQRT_2; 2];
const Q_LR8: [f64; 4] = [
    1.306_562_964_876_376_4,
    0.541_196_100_146_197,
    1.306_562_964_876_376_4,
    0.541_196_100_146_197,
];

/// EQ Three's device type.
#[must_use]
pub fn three_band_eq_type() -> DeviceType {
    let gain =
        |id, name: &str, text: &str| ParamDescriptor::gain(id, name, TOP_DB, 0.0).describe(text);
    let on = |id, name: &str, text: &str| ParamDescriptor::toggle(id, name, true).describe(text);
    let params = vec![
        gain(
            GAIN_LOW,
            "Gain Low",
            "The low band's level, from silence to +6 dB.",
        ),
        gain(
            GAIN_MID,
            "Gain Mid",
            "The mid band's level, from silence to +6 dB.",
        ),
        gain(
            GAIN_HIGH,
            "Gain High",
            "The high band's level, from silence to +6 dB.",
        ),
        on(ON_LOW, "On Low", "Switches the low band on or off."),
        on(ON_MID, "On Mid", "Switches the mid band on or off."),
        on(ON_HIGH, "On High", "Switches the high band on or off."),
        ParamDescriptor::continuous(
            FREQ_LOW,
            "FreqLo",
            FREQ_LOW_RANGE,
            FREQ_LOW_DEFAULT,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("Where the low band ends and the mid band begins."),
        ParamDescriptor::continuous(
            FREQ_HIGH,
            "FreqHi",
            FREQ_HIGH_RANGE,
            FREQ_HIGH_DEFAULT,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("Where the mid band ends and the high band begins."),
        ParamDescriptor::choice(SLOPE, "Slope", &SLOPES, 0)
            .describe("How steeply the bands part: 24 or 48 dB per octave."),
    ];
    DeviceType::native(THREE_BAND_EQ, "EQ Three", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&[
                (
                    "Bands",
                    &[
                        "Gain Low",
                        "Gain Mid",
                        "Gain High",
                        "On Low",
                        "On Mid",
                        "On High",
                    ],
                ),
                ("Crossover", &["FreqLo", "FreqHi", "Slope"]),
            ]),
    )
}

/// The three bands as columns, low to high, and the crossovers beside them.
fn layout() -> DeviceLayout {
    let column = |gain, on| {
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(gain, Widget::Knob), (on, Widget::Toggle)],
        ))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        column(GAIN_LOW, ON_LOW),
        column(GAIN_MID, ON_MID),
        column(GAIN_HIGH, ON_HIGH),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (FREQ_LOW, Widget::Knob),
                (FREQ_HIGH, Widget::Knob),
                (SLOPE, Widget::Chooser),
            ],
        )),
    ]))
}

/// Adds EQ Three to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, three_band_eq_type(), |_, config| {
        Triad::new(config)
    });
}

/// One crossover's coefficients: a low-pass and a high-pass, `stages`
/// sections each.
#[derive(Clone, Copy, Default)]
struct Crossover {
    low_pass: [Coeffs; MAX_STAGES],
    high_pass: [Coeffs; MAX_STAGES],
    stages: usize,
}

impl Crossover {
    fn new(hz: f64, steep: bool, rate: f64) -> Self {
        let qs: &[f64] = if steep { &Q_LR8 } else { &Q_LR4 };
        let mut out = Self {
            stages: qs.len(),
            ..Self::default()
        };
        for (i, q) in qs.iter().enumerate() {
            out.low_pass[i] = coeffs(Type::HighCut12, hz, 0.0, *q, rate);
            out.high_pass[i] = coeffs(Type::LowCut12, hz, 0.0, *q, rate);
        }
        out
    }
}

/// One channel's filters for one crossover.
#[derive(Clone, Copy, Default)]
struct Split {
    low_pass: [Biquad; MAX_STAGES],
    high_pass: [Biquad; MAX_STAGES],
    stages: usize,
}

impl Split {
    fn set(&mut self, crossover: &Crossover) {
        self.stages = crossover.stages;
        for i in 0..MAX_STAGES {
            self.low_pass[i].c = crossover.low_pass[i];
            self.high_pass[i].c = crossover.high_pass[i];
        }
    }

    #[inline]
    fn low(&mut self, x: f32) -> f32 {
        self.low_pass[..self.stages]
            .iter_mut()
            .fold(x, |y, s| s.process(y))
    }

    #[inline]
    fn high(&mut self, x: f32) -> f32 {
        self.high_pass[..self.stages]
            .iter_mut()
            .fold(x, |y, s| s.process(y))
    }
}

/// One channel's crossovers.
#[derive(Clone, Copy, Default)]
struct Chain {
    /// The crossover at FreqLo.
    lower: Split,
    /// The crossover at FreqHi.
    upper: Split,
    /// The low band's all-pass at FreqHi (24 dB only): the upper
    /// crossover's low-pass and high-pass added.
    align: Split,
}

/// EQ Three's audio-thread half.
pub struct Triad {
    rate: f64,
    chains: [Chain; 2],
    steep: bool,
    /// The crossover frequencies the chains were designed for.
    last: Option<(f64, f64)>,
    tick: usize,
    /// Each channel's band levels, as peak followers, and their decay per
    /// sample.
    levels: [[f32; 3]; 2],
    decay: f32,
}

impl Triad {
    /// An EQ Three for `config`.
    #[must_use]
    pub fn new(config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        Self {
            rate,
            chains: [Chain::default(); 2],
            steep: false,
            last: None,
            tick: 0,
            levels: [[0.0; 3]; 2],
            decay: (-1.0 / (LIGHT_RELEASE_SECONDS * rate)).exp() as f32,
        }
    }

    /// Whether each band's signal light is lit: its level is above
    /// [`LIGHT_THRESHOLD_DB`], on or off.
    #[must_use]
    pub fn lights(&self) -> [bool; 3] {
        let threshold = 10f32.powf(LIGHT_THRESHOLD_DB as f32 / 20.0);
        core::array::from_fn(|band| self.levels.iter().any(|c| c[band] > threshold))
    }
}

impl NativeDevice for Triad {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        let at = |id| params.index(id).expect("the description holds it");
        let steep = params.value(at(SLOPE)) >= 0.5;
        if steep != self.steep {
            self.steep = steep;
            self.chains = [Chain::default(); 2];
            self.last = None;
            self.tick = 0;
        }
        let gains = [GAIN_LOW, GAIN_MID, GAIN_HIGH].map(|id| params.ramp(at(id)));
        let on = [ON_LOW, ON_MID, ON_HIGH].map(|id| params.value(at(id)) >= 0.5);
        let (low_hz, high_hz) = (params.ramp(at(FREQ_LOW)), params.ramp(at(FREQ_HIGH)));
        let mut i = 0;
        while i < frames {
            if self.tick.is_multiple_of(REFRESH_FRAMES) {
                // The upper crossover never sits below the lower one.
                let lower_hz = f64::from(low_hz[i]);
                let upper_hz = f64::from(high_hz[i]).max(lower_hz);
                if self.last != Some((lower_hz, upper_hz)) {
                    self.last = Some((lower_hz, upper_hz));
                    let lower = Crossover::new(lower_hz, steep, self.rate);
                    let upper = Crossover::new(upper_hz, steep, self.rate);
                    for chain in &mut self.chains {
                        chain.lower.set(&lower);
                        chain.upper.set(&upper);
                        chain.align.set(&upper);
                    }
                }
            }
            let n = (REFRESH_FRAMES - self.tick % REFRESH_FRAMES).min(frames - i);
            for channel in 0..channels {
                let chain = &mut self.chains[channel];
                let levels = &mut self.levels[channel];
                for (k, sample) in audio.channel_mut(channel)[i..i + n].iter_mut().enumerate() {
                    let x = *sample;
                    let rest = chain.lower.high(x);
                    let mut low = chain.lower.low(x);
                    if !steep {
                        low = chain.align.low(low) + chain.align.high(low);
                    }
                    let bands = [low, chain.upper.low(rest), chain.upper.high(rest)];
                    let mut y = 0.0;
                    for band in 0..3 {
                        levels[band] = bands[band].abs().max(levels[band] * self.decay);
                        if on[band] {
                            y += bands[band] * gains[band][i + k];
                        }
                    }
                    *sample = y;
                }
            }
            self.tick += n;
            i += n;
        }
    }

    fn display_len(&self) -> usize {
        3
    }

    fn display(&mut self, out: &mut [f32]) {
        for (value, lit) in out.iter_mut().zip(self.lights()) {
            *value = f32::from(u8::from(lit));
        }
    }
}

/// The device's display (the three-band-eq spec): the three signal lights
/// as readings, once an engine publishes them. `data` is what the node
/// published, if an engine runs.
#[must_use]
pub fn display(
    device: DeviceId,
    _description: &DeviceDescription,
    _values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let readings = data
        .filter(|d| d.len() == 3)
        .map(|lights| {
            ["Low", "Mid", "High"]
                .iter()
                .zip(lights)
                .map(|(band, lit)| DisplayReading {
                    name: format!("{band} Signal"),
                    value: *lit,
                    unit: String::new(),
                    text: if *lit >= 0.5 { "lit" } else { "off" }.to_owned(),
                })
                .collect()
        })
        .unwrap_or_default();
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings,
    }
}
