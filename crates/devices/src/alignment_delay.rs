//! Plumb, the alignment delay (WP-5.25; the alignment delay spec): a
//! whole-sample delay for each side, set in milliseconds, in samples, or as
//! the distance sound travels, with no latency reported.

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};

/// Plumb's device type.
pub const ALIGNMENT_DELAY: &str = "bl.device.plumb";

/// Delay Mode: Time, Samples, or Distance.
pub const MODE: ParamId = ParamId(0);
/// Link: the left delay for both sides.
pub const LINK: ParamId = ParamId(1);
/// The unit of a distance: metres or feet.
pub const DISTANCE_UNIT: ParamId = ParamId(2);
/// The unit of the temperature: °C or °F.
pub const TEMPERATURE_UNIT: ParamId = ParamId(3);
/// The air's temperature in °C, used with the metric unit.
pub const TEMPERATURE_C: ParamId = ParamId(4);
/// The air's temperature in °F, used with the other.
pub const TEMPERATURE_F: ParamId = ParamId(5);

/// A side's delay in milliseconds, as its offset among the side's four.
pub const TIME: u32 = 0;
/// A side's delay in samples.
pub const SAMPLES: u32 = 1;
/// A side's delay as a distance in metres.
pub const METRES: u32 = 2;
/// A side's delay as a distance in feet.
pub const FEET: u32 = 3;

/// Parameters before the sides', and in each side.
const COMMON: u32 = 6;
const FIELDS: u32 = 4;
/// How many parameters the device has.
const PARAMS: usize = COMMON as usize + 2 * FIELDS as usize;

/// The longest delay in samples, whatever the sample rate.
pub const MAX_SAMPLES: usize = 96_000;
/// The longest delay in seconds, at rates up to [`MAX_SAMPLES`] a second.
const MAX_SECONDS: f64 = 1.0;
/// The speed of sound in dry air at 0 °C, in metres a second, and how much
/// it gains for each degree (Beatlayer's own formula, the spec's R3).
const SOUND_AT_ZERO: f64 = 331.3;
const SOUND_PER_DEGREE: f64 = 0.606;
const METRES_PER_FOOT: f64 = 0.3048;

/// Parameter `field` of side `side` (0 left, 1 right).
#[must_use]
pub const fn side_param(side: usize, field: u32) -> ParamId {
    ParamId(COMMON + side as u32 * FIELDS + field)
}

/// The speed of sound, in metres a second, at `celsius`.
#[must_use]
pub fn speed_of_sound(celsius: f64) -> f64 {
    SOUND_AT_ZERO + SOUND_PER_DEGREE * celsius
}

/// Plumb's type: Beatlayer's own name and look, Live's modes (the alignment
/// delay spec).
#[must_use]
pub fn alignment_delay_type() -> DeviceType {
    let mut params = vec![
        ParamDescriptor::choice(MODE, "Delay Mode", &["Time", "Samples", "Distance"], 0)
            .describe("Sets the delay in milliseconds, in samples, or as a distance."),
        ParamDescriptor::toggle(LINK, "Link", false)
            .describe("Gives both sides the left side's delay."),
        ParamDescriptor::choice(DISTANCE_UNIT, "Distance Unit", &["m", "ft"], 0)
            .describe("Measures a distance in metres or in feet."),
        ParamDescriptor::choice(TEMPERATURE_UNIT, "Temperature Unit", &["°C", "°F"], 0)
            .describe("Measures the temperature in degrees Celsius or Fahrenheit."),
        ParamDescriptor::continuous(
            TEMPERATURE_C,
            "Temperature °C",
            (-30.0, 50.0),
            20.0,
            ParamUnit::None,
        )
        .describe("The air's temperature in °C, which sets the speed of sound in Distance mode."),
        ParamDescriptor::continuous(
            TEMPERATURE_F,
            "Temperature °F",
            (-22.0, 122.0),
            68.0,
            ParamUnit::None,
        )
        .describe("The air's temperature in °F, which sets the speed of sound in Distance mode."),
    ];
    for (side, name) in ["Left", "Right"].into_iter().enumerate() {
        params.extend([
            ParamDescriptor::continuous(
                side_param(side, TIME),
                &format!("{name} Time"),
                (0.0, 1_000.0),
                0.0,
                ParamUnit::Milliseconds,
            )
            .describe("The side's delay in milliseconds."),
            ParamDescriptor::integer(
                side_param(side, SAMPLES),
                &format!("{name} Samples"),
                (0, MAX_SAMPLES as i64),
                0,
                ParamUnit::None,
            )
            .describe("The side's delay in samples."),
            ParamDescriptor::continuous(
                side_param(side, METRES),
                &format!("{name} Metres"),
                (0.0, 300.0),
                0.0,
                ParamUnit::None,
            )
            .describe("The side's delay as the distance in metres sound covers in that time."),
            ParamDescriptor::continuous(
                side_param(side, FEET),
                &format!("{name} Feet"),
                (0.0, 300.0),
                0.0,
                ParamUnit::None,
            )
            .describe("The side's delay as the distance in feet sound covers in that time."),
        ]);
    }
    DeviceType::native(ALIGNMENT_DELAY, "Plumb", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

fn layout() -> DeviceLayout {
    let side = |side| {
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (side_param(side, TIME), Widget::Knob),
                (side_param(side, SAMPLES), Widget::Number),
                (side_param(side, METRES), Widget::Knob),
                (side_param(side, FEET), Widget::Knob),
            ],
        ))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (MODE, Widget::Chooser),
                (LINK, Widget::Toggle),
                (DISTANCE_UNIT, Widget::Chooser),
                (TEMPERATURE_UNIT, Widget::Chooser),
                (TEMPERATURE_C, Widget::Knob),
                (TEMPERATURE_F, Widget::Knob),
            ],
        )),
        side(0),
        side(1),
    ]))
}

/// Adds Plumb to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, alignment_delay_type(), Plumb::new);
}

/// Plumb: one delay line a channel.
struct Plumb {
    lines: Vec<DelayLine>,
    max: usize,
}

impl Plumb {
    fn new(_: &Device, config: &NodeConfig) -> Self {
        let max = MAX_SAMPLES.max((MAX_SECONDS * config.sample_rate.as_f64()).ceil() as usize);
        Self {
            lines: (0..config.channels).map(|_| DelayLine::new(max)).collect(),
            max,
        }
    }
}

impl NativeDevice for Plumb {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let rate = audio.sample_rate().as_f64();
        let ix: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let ramp = |id: ParamId| params.ramp(ix[id.0 as usize]);
        let choice = |id: ParamId| params.value(ix[id.0 as usize]) as usize;
        let (mode, link, feet, fahrenheit) = (
            choice(MODE),
            choice(LINK) == 1,
            choice(DISTANCE_UNIT) == 1,
            choice(TEMPERATURE_UNIT) == 1,
        );
        let celsius = if fahrenheit {
            ramp(TEMPERATURE_F)
        } else {
            ramp(TEMPERATURE_C)
        };
        let max = self.max;
        for (channel, samples) in audio.channels_mut().enumerate() {
            // Left is the first channel, right every other; Link gives
            // right the left's delay.
            let side = usize::from(channel > 0 && !link);
            let time = ramp(side_param(side, TIME));
            let count = ramp(side_param(side, SAMPLES));
            let distance = ramp(side_param(side, if feet { FEET } else { METRES }));
            let line = &mut self.lines[channel];
            for (i, sample) in samples.iter_mut().enumerate() {
                let delay = match mode {
                    0 => f64::from(time[i]) * rate / 1_000.0,
                    1 => f64::from(count[i]),
                    _ => {
                        let temperature = f64::from(celsius[i]);
                        let celsius = if fahrenheit {
                            (temperature - 32.0) * 5.0 / 9.0
                        } else {
                            temperature
                        };
                        let metres =
                            f64::from(distance[i]) * if feet { METRES_PER_FOOT } else { 1.0 };
                        metres / speed_of_sound(celsius) * rate
                    }
                };
                let delay = (delay.round() as usize).min(max);
                let input = *sample;
                // A delay of none passes the input: the line reads what was
                // written before this sample.
                if delay > 0 {
                    *sample = line.read(delay);
                }
                line.write(input);
            }
        }
    }
}
