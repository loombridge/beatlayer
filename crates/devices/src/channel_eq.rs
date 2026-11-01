//! Channel EQ, the native three-band channel EQ (WP-5.21; the channel-eq spec):
//! an 80 Hz high-pass switch, a low shelf at 100 Hz, a sweepable mid peak,
//! and a high band that is a shelf when it boosts and a shelf with a
//! low-pass when it cuts, then Output. Its display holds the response and
//! the output's spectrum.
//!
//! The sections are EQ Eight's (RBJ's audio-EQ cookbook) and the spectrum
//! is EQ Eight's analyser. The band frequencies, gains and the high band's
//! cut behaviour are from the manual's section on Channel EQ; the shelf's
//! gain-dependent shape, the mid band's Q, the high shelf's corner and the
//! high-pass's order are Beatlayer's own (see `docs/specs/channel-eq.md`).

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker, OrdMap,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::eq::{Type, coeffs};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::sections::{
    ANALYSER_BANDS, Analyser, Biquad, Coeffs, DISPLAY_RATE, REFRESH_FRAMES, curve_freqs,
    curve_series, response_db, spectrum_series,
};

/// Channel EQ's device type.
pub const CHANNEL_EQ: &str = "beatlayer.strip";

/// HP 80: the 80 Hz high-pass switch.
pub const HIGH_PASS: ParamId = ParamId(0);
/// Low: the low shelf's gain, in dB.
pub const LOW: ParamId = ParamId(1);
/// Mid: the mid peak's gain, in dB.
pub const MID: ParamId = ParamId(2);
/// Mid Freq: the mid peak's centre, in Hz.
pub const MID_FREQ: ParamId = ParamId(3);
/// High: the high band's gain, in dB.
pub const HIGH: ParamId = ParamId(4);
/// Output: the level after the bands, a gain reaching silence.
pub const OUTPUT: ParamId = ParamId(5);

/// The high-pass's corner (the -3 dB point), in Hz.
pub const HIGH_PASS_HZ: f64 = 80.0;
/// The low shelf's corner (its midpoint), in Hz.
pub const LOW_SHELF_HZ: f64 = 100.0;
/// The mid band's frequency range and default, in Hz.
pub const MID_MIN_HZ: f64 = 120.0;
pub const MID_MAX_HZ: f64 = 7_500.0;
pub const MID_DEFAULT_HZ: f64 = 1_000.0;
/// The high shelf's corner (its midpoint), in Hz. Beatlayer's own.
pub const HIGH_SHELF_HZ: f64 = 6_000.0;
/// The gains' ranges, in dB.
pub const LOW_RANGE_DB: f64 = 15.0;
pub const MID_RANGE_DB: f64 = 12.0;
pub const HIGH_RANGE_DB: f64 = 15.0;
/// The high band's low-pass corner at no cut and at a full cut, in Hz; the
/// corner moves between them evenly on a logarithmic axis.
pub const LOW_PASS_TOP_HZ: f64 = 20_000.0;
pub const LOW_PASS_BOTTOM_HZ: f64 = 8_000.0;
/// The mid band's Q. Beatlayer's own.
pub const MID_Q: f64 = 0.8;
/// The low shelf's Q at no gain and at full gain: the curve steepens as the
/// gain rises. Beatlayer's own.
pub const SHELF_Q_FLAT: f64 = 0.5;
pub const SHELF_Q_FULL: f64 = 0.71;
/// Output's top, in dB. Beatlayer's own.
const OUTPUT_MAX_DB: f64 = 12.0;

/// The low shelf's Q at `gain_db`.
#[must_use]
pub fn low_shelf_q(gain_db: f64) -> f64 {
    SHELF_Q_FLAT + (SHELF_Q_FULL - SHELF_Q_FLAT) * (gain_db.abs() / LOW_RANGE_DB).min(1.0)
}

/// The high band's low-pass corner at `gain_db` (below 0), in Hz.
#[must_use]
pub fn low_pass_hz(gain_db: f64) -> f64 {
    let cut = (-gain_db / HIGH_RANGE_DB).clamp(0.0, 1.0);
    LOW_PASS_TOP_HZ * (LOW_PASS_BOTTOM_HZ / LOW_PASS_TOP_HZ).powf(cut)
}

/// Channel EQ's device type.
#[must_use]
pub fn channel_eq_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::toggle(HIGH_PASS, "HP 80", false)
            .describe("Cuts everything below 80 Hz, for rumble."),
        ParamDescriptor::continuous(
            LOW,
            "Low",
            (-LOW_RANGE_DB, LOW_RANGE_DB),
            0.0,
            ParamUnit::Decibels,
        )
        .describe("Boosts or cuts the lows with a shelf at 100 Hz."),
        ParamDescriptor::continuous(
            MID,
            "Mid",
            (-MID_RANGE_DB, MID_RANGE_DB),
            0.0,
            ParamUnit::Decibels,
        )
        .describe("Boosts or cuts the mids with a peak at the Mid Freq."),
        ParamDescriptor::continuous(
            MID_FREQ,
            "Mid Freq",
            (MID_MIN_HZ, MID_MAX_HZ),
            MID_DEFAULT_HZ,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("Where the mid peak sits."),
        ParamDescriptor::continuous(
            HIGH,
            "High",
            (-HIGH_RANGE_DB, HIGH_RANGE_DB),
            0.0,
            ParamUnit::Decibels,
        )
        .describe(
            "Boosts the highs with a shelf; cuts them with a shelf and a low-pass whose \
             corner falls as the cut deepens.",
        ),
        ParamDescriptor::gain(OUTPUT, "Output", OUTPUT_MAX_DB, 0.0)
            .describe("The level after the bands, to offset what the EQ added."),
    ];
    DeviceType::native(CHANNEL_EQ, "Channel EQ", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&[(
                "Channel EQ",
                &["Low", "Mid", "Mid Freq", "High", "HP 80", "Output"],
            )]),
    )
}

/// The controls on the left, the display growing to their right.
fn layout() -> DeviceLayout {
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (HIGH_PASS, Widget::Toggle),
                (LOW, Widget::Knob),
                (MID, Widget::Knob),
                (MID_FREQ, Widget::Knob),
                (HIGH, Widget::Knob),
                (OUTPUT, Widget::Knob),
            ],
        )),
        LayoutNode::Cell(DisplayCell::new("EQ Curve", 150.0, BODY_HEIGHT, 1.0)),
    ]))
}

/// Adds Channel EQ to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, channel_eq_type(), |_, config| Strip::new(config));
}

/// The sections' coefficients for one set of values.
#[derive(Clone, Copy)]
struct Plan {
    high_pass: Coeffs,
    low: Coeffs,
    mid: Coeffs,
    high: Coeffs,
    low_pass: Coeffs,
}

/// A section that leaves its input as it is.
const THROUGH: Coeffs = Coeffs {
    b0: 1.0,
    b1: 0.0,
    b2: 0.0,
    a1: 0.0,
    a2: 0.0,
};

impl Plan {
    fn new(
        high_pass: bool,
        low_db: f64,
        mid_db: f64,
        mid_hz: f64,
        high_db: f64,
        rate: f64,
    ) -> Self {
        let butterworth = std::f64::consts::FRAC_1_SQRT_2;
        Self {
            high_pass: if high_pass {
                coeffs(Type::LowCut12, HIGH_PASS_HZ, 0.0, butterworth, rate)
            } else {
                THROUGH
            },
            low: coeffs(
                Type::LowShelf,
                LOW_SHELF_HZ,
                low_db,
                low_shelf_q(low_db),
                rate,
            ),
            mid: coeffs(Type::Peak, mid_hz, mid_db, MID_Q, rate),
            high: coeffs(Type::HighShelf, HIGH_SHELF_HZ, high_db, butterworth, rate),
            low_pass: if high_db < 0.0 {
                coeffs(
                    Type::HighCut12,
                    low_pass_hz(high_db),
                    0.0,
                    butterworth,
                    rate,
                )
            } else {
                THROUGH
            },
        }
    }

    fn sections(&self) -> [Coeffs; 5] {
        [self.high_pass, self.low, self.mid, self.high, self.low_pass]
    }
}

/// One channel's sections, in order.
#[derive(Clone, Copy, Default)]
struct Chain([Biquad; 5]);

impl Chain {
    fn set(&mut self, plan: &Plan) {
        for (section, c) in self.0.iter_mut().zip(plan.sections()) {
            section.set(c);
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.0.iter_mut().fold(x, |y, section| section.process(y))
    }
}

/// Channel EQ's audio-thread half.
pub struct Strip {
    rate: f64,
    chains: [Chain; 2],
    plan: Plan,
    /// The values the plan was worked out for.
    last: Option<(bool, [f64; 4])>,
    /// Frames since the device was made, so the sections are redesigned on
    /// the same frames at any block size.
    tick: usize,
    analyser: Analyser,
}

impl Strip {
    /// A Channel EQ for `config`, every buffer made here.
    #[must_use]
    pub fn new(config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        Self {
            rate,
            chains: [Chain::default(); 2],
            plan: Plan::new(false, 0.0, 0.0, MID_DEFAULT_HZ, 0.0, rate),
            last: None,
            tick: 0,
            analyser: Analyser::new(rate),
        }
    }

    /// The analyser's last window, in dB per band, for a headless caller.
    #[must_use]
    pub fn analyser(&self) -> &[f32; ANALYSER_BANDS] {
        self.analyser.bands()
    }
}

impl NativeDevice for Strip {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        let at = |id| params.index(id).expect("the description holds it");
        let (low, mid, mid_hz, high, output) = (
            params.ramp(at(LOW)),
            params.ramp(at(MID)),
            params.ramp(at(MID_FREQ)),
            params.ramp(at(HIGH)),
            params.ramp(at(OUTPUT)),
        );
        let high_pass = params.value(at(HIGH_PASS)) >= 0.5;
        let mut i = 0;
        while i < frames {
            if self.tick.is_multiple_of(REFRESH_FRAMES) {
                let values = [low[i], mid[i], mid_hz[i], high[i]].map(f64::from);
                if self.last != Some((high_pass, values)) {
                    self.last = Some((high_pass, values));
                    let [low, mid, mid_hz, high] = values;
                    self.plan = Plan::new(high_pass, low, mid, mid_hz, high, self.rate);
                    for chain in &mut self.chains {
                        chain.set(&self.plan);
                    }
                }
            }
            let n = (REFRESH_FRAMES - self.tick % REFRESH_FRAMES).min(frames - i);
            for channel in 0..channels {
                let chain = &mut self.chains[channel];
                for (sample, level) in audio.channel_mut(channel)[i..i + n]
                    .iter_mut()
                    .zip(&output[i..i + n])
                {
                    *sample = chain.process(*sample) * level;
                }
            }
            self.tick += n;
            i += n;
        }
        self.analyser.push_mix(audio);
    }

    fn display_len(&self) -> usize {
        ANALYSER_BANDS
    }

    fn display(&mut self, out: &mut [f32]) {
        out.copy_from_slice(self.analyser.bands());
    }
}

/// The levels the display's response shows between its bottom and its top,
/// in dB.
const CURVE_RANGE_DB: [f32; 2] = [-18.0, 18.0];

/// The device's display (the channel-eq spec): the response of the bands
/// together, a marker at the mid peak, and the output's spectrum once an
/// engine publishes one. `data` is what the node published, if an engine
/// runs.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let text = |id: ParamId, v: f64| {
        description
            .param(id)
            .map_or_else(String::new, |p| p.format(v))
    };
    let (mid_db, mid_hz) = (value(MID), value(MID_FREQ));
    let plan = Plan::new(
        value(HIGH_PASS) >= 0.5,
        value(LOW),
        mid_db,
        mid_hz,
        value(HIGH),
        DISPLAY_RATE,
    );
    let response: Vec<f32> = curve_freqs()
        .iter()
        .map(|&hz| {
            plan.sections()
                .iter()
                .map(|c| response_db(*c, hz, DISPLAY_RATE))
                .sum::<f64>() as f32
        })
        .collect();
    let mut out = DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    };
    if let Some(data) = data {
        out.series.push(spectrum_series(data));
    }
    out.series.push(curve_series(
        "Response".to_owned(),
        CURVE_RANGE_DB,
        false,
        response,
    ));
    out.markers.push(DisplayMarker {
        handle: None,
        name: "Mid".to_owned(),
        series: "Response".to_owned(),
        x: mid_hz as f32,
        y: mid_db as f32,
        text: format!("Mid {} {}", text(MID_FREQ, mid_hz), text(MID, mid_db)),
    });
    out
}
