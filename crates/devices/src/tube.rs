//! Dynamic Tube, the native tube distortion (WP-5.23; the tube distortion spec):
//! three tube models, a Tone control that steers where the distortion falls,
//! a Bias that pushes the signal into the curve, and an envelope that moves
//! the Bias with the input's level.
//!
//! A tube is a curve that is linear up to a knee and eases to 1 above it,
//! worked at an operating point (the bias): the output is the curve at the
//! signal plus the operating point, less the curve at the operating point
//! alone. Model A has a high knee and no built-in offset, so it is clean
//! until the signal reaches the knee; C has no knee and an offset of its
//! own, so it distorts at every level; B is between. Tone is a shelf before
//! the curve and its inverse after it, so it changes only what the curve
//! hears. Hi-Quality runs the curve at 8 times the rate. The models' knees
//! and offsets are Beatlayer's own: the manual describes the models but
//! gives no numbers.

use bl_dsp::{EnvelopeFollower, Oversampler};
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::db_to_gain;
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker,
    DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::eq::{Type, coeffs};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::saturator::soft_knee;
use crate::sections::Biquad;

/// Dynamic Tube's device type.
pub const TUBE: &str = "beatlayer.valve";

/// Model: A, B or C.
pub const MODEL: ParamId = ParamId(0);
/// Tone, per cent: the distortion toward the lows at 0, toward the highs at
/// 100.
pub const TONE: ParamId = ParamId(1);
/// Drive: the gain before the tube, in dB.
pub const DRIVE: ParamId = ParamId(2);
/// Bias, per cent: how far the signal sits into the curve.
pub const BIAS: ParamId = ParamId(3);
/// Envelope, per cent: how much the input's level moves the Bias.
pub const ENVELOPE: ParamId = ParamId(4);
/// Attack: the envelope's rise time constant, in ms.
pub const ATTACK: ParamId = ParamId(5);
/// Release: the envelope's fall time constant, in ms.
pub const RELEASE: ParamId = ParamId(6);
/// Output: the level after the tube.
pub const OUTPUT: ParamId = ParamId(7);
/// Hi-Quality, a menu item.
pub const HI_QUALITY: ParamId = ParamId(8);

/// The models' knees and built-in offsets, A to C.
pub const MODELS: [&str; 3] = ["A", "B", "C"];
const KNEES: [f32; 3] = [0.5, 0.2, 0.0];
const OFFSETS: [f32; 3] = [0.0, 0.05, 0.15];

/// Tone: the shelf's corner, and its gain at 0 % and 100 % (the negative of
/// it and itself), in dB.
const TONE_HZ: f64 = 1_500.0;
const TONE_RANGE_DB: f64 = 12.0;
/// Frames between coefficient updates, counted from the device's first
/// frame, so a render doesn't depend on its block size.
const REFRESH_FRAMES: u64 = 16;
/// Hi-Quality: 8 times the rate.
const HI_QUALITY_STAGES: u32 = 3;

/// The display's curve: points across inputs from -1 to 1.
pub const CURVE_POINTS: usize = 97;
const LEVEL_FLOOR_DB: f32 = -120.0;

/// The tube's output for `x` (after Drive) at operating point `offset`.
fn tube(model: usize, offset: f32, x: f32) -> f32 {
    let knee = KNEES[model];
    soft_knee(x + offset, knee) - soft_knee(offset, knee)
}

/// The operating point for `model` at `bias`, a fraction from 0 to 1.
fn operating_point(model: usize, bias: f32) -> f32 {
    OFFSETS[model] + bias.clamp(0.0, 1.0)
}

/// The Dynamic Tube device type.
#[must_use]
pub fn tube_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::choice(MODEL, "Tube", &MODELS, 0).describe(
            "The tube's character: A is clean until the signal reaches a threshold, C always \
             distorts, and B is between.",
        ),
        ParamDescriptor::continuous(TONE, "Tone", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("Steers the distortion toward the lows and mids (low) or the highs (high)."),
        ParamDescriptor::continuous(DRIVE, "Drive", (0.0, 36.0), 6.0, ParamUnit::Decibels)
            .describe("How much signal reaches the tube; more is dirtier."),
        ParamDescriptor::continuous(BIAS, "Bias", (0.0, 100.0), 20.0, ParamUnit::Percent)
            .describe("Pushes the signal into the tube's curve; very high values break it up."),
        ParamDescriptor::continuous(ENVELOPE, "Envelope", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe(
                "How much the input's level moves the Bias: above 0 loud sounds get dirtier, \
                 below 0 they get cleaner.",
            ),
        ParamDescriptor::continuous(ATTACK, "Attack", (0.1, 1_000.0), 10.0, ParamUnit::Milliseconds)
            .logarithmic()
            .describe("How quickly the envelope follows a rise in level; it does nothing at Envelope 0."),
        ParamDescriptor::continuous(RELEASE, "Release", (1.0, 5_000.0), 100.0, ParamUnit::Milliseconds)
            .logarithmic()
            .describe("How quickly the envelope follows a fall in level; it does nothing at Envelope 0."),
        ParamDescriptor::gain(OUTPUT, "Output", 6.0, 0.0)
            .describe("The level after the tube."),
        ParamDescriptor::toggle(HI_QUALITY, "Hi-Quality", false)
            .describe("Runs the tube at eight times the rate so its harmonics don't alias; it delays the output."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(MODEL, Widget::Chooser), (TONE, Widget::Knob)],
        )),
        LayoutNode::Cell(
            DisplayCell::new("Tube Curve", 130.0, BODY_HEIGHT, 1.0)
                .with_bottom(ControlBand::stacked(60.0, &[(DRIVE, Widget::Knob)])),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (BIAS, Widget::Knob),
                (ENVELOPE, Widget::Knob),
                (ATTACK, Widget::Knob),
                (RELEASE, Widget::Knob),
                (OUTPUT, Widget::Knob),
            ],
        )),
    ]));
    DeviceType::native(TUBE, "Dynamic Tube", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_menu_options(&[HI_QUALITY])
            .with_banks(&[
                ("Valve", &["Tube", "Tone", "Drive", "Bias", "Output"]),
                ("Envelope", &["Envelope", "Attack", "Release"]),
            ]),
    )
}

/// Adds Dynamic Tube to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, tube_type(), make);
}

/// Makes a Dynamic Tube for `device`'s values and `config`'s rate, channels and
/// block size. Every buffer is made here, off the audio thread, and its
/// latency comes from the set's Hi-Quality value, so it is reported before
/// the device first runs.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> Valve {
    Valve::new(device, config)
}

/// Dynamic Tube's audio-thread half.
pub struct Valve {
    rate: f64,
    /// The input's level, one follower per channel.
    followers: [EnvelopeFollower; 2],
    tone_before: [Biquad; 2],
    tone_after: [Biquad; 2],
    /// The signal on its way through the tube, and the operating point at
    /// each frame.
    work: [Vec<f32>; 2],
    offsets: Vec<f32>,
    oversamplers: [Oversampler; 2],
    /// Frames processed so far, which the Tone refresh counts.
    ticks: u64,
    hi_quality: bool,
    latency: u32,
    /// The input's peak since the last block, for the display.
    peak: f32,
}

impl Valve {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let frames = config.max_frames.max(1);
        let oversampler = || Oversampler::new(HI_QUALITY_STAGES, config.channels.max(1), frames);
        let oversamplers = [oversampler(), oversampler()];
        let hi_quality = device.params.get(&HI_QUALITY).copied().unwrap_or(0.0) >= 0.5;
        // A shelf that does nothing until the first refresh sets it.
        let mut flat = Biquad::default();
        flat.c = coeffs(Type::HighShelf, TONE_HZ, 0.0, 0.71, rate);
        Self {
            rate,
            followers: [EnvelopeFollower::new(rate), EnvelopeFollower::new(rate)],
            tone_before: [flat; 2],
            tone_after: [flat; 2],
            work: [vec![0.0; frames], vec![0.0; frames]],
            offsets: vec![0.0; frames],
            latency: if hi_quality {
                oversamplers[0].latency()
            } else {
                0
            },
            oversamplers,
            ticks: 0,
            hi_quality,
            peak: 0.0,
        }
    }
}

impl NativeDevice for Valve {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let model = (params.value(MODEL.0 as usize) as usize).min(MODELS.len() - 1);
        let hi_quality = params.value(HI_QUALITY.0 as usize) >= 0.5;
        if hi_quality && !self.hi_quality {
            for oversampler in &mut self.oversamplers {
                oversampler.reset();
            }
        }
        self.hi_quality = hi_quality;
        self.latency = if hi_quality {
            self.oversamplers[0].latency()
        } else {
            0
        };
        for follower in &mut self.followers {
            follower.set(
                params.target(ATTACK.0 as usize) / 1_000.0,
                params.target(RELEASE.0 as usize) / 1_000.0,
                false,
            );
        }
        let ramp = |id: ParamId| params.ramp(id.0 as usize);
        let (tone, drive, bias, envelope, output) = (
            ramp(TONE),
            ramp(DRIVE),
            ramp(BIAS),
            ramp(ENVELOPE),
            ramp(OUTPUT),
        );

        // 1. Per frame: the envelope moves the operating point, and the
        // input goes through Tone's shelf and the Drive gain.
        for i in 0..frames {
            if (self.ticks + i as u64).is_multiple_of(REFRESH_FRAMES) {
                let gain = TONE_RANGE_DB * (f64::from(tone[i]) - 50.0) / 50.0;
                for channel in 0..channels {
                    self.tone_before[channel].c =
                        coeffs(Type::HighShelf, TONE_HZ, gain, 0.71, self.rate);
                    self.tone_after[channel].c =
                        coeffs(Type::HighShelf, TONE_HZ, -gain, 0.71, self.rate);
                }
            }
            let drive_gain = db_to_gain(f64::from(drive[i])) as f32;
            let mut level = 0.0f32;
            for channel in 0..channels {
                let x = audio.channel_mut(channel)[i];
                self.peak = self.peak.max(x.abs());
                level = level.max(self.followers[channel].process(x) as f32);
                self.work[channel][i] = self.tone_before[channel].process(x) * drive_gain;
            }
            self.offsets[i] = operating_point(model, (bias[i] + envelope[i] * level) / 100.0);
        }

        // 2. The tube, at the rate or at 8 times it.
        for channel in 0..channels {
            let work = &mut self.work[channel][..frames];
            let offsets = &self.offsets[..frames];
            if hi_quality {
                let factor = self.oversamplers[channel].factor();
                let mut frame = 0;
                self.oversamplers[channel].process(channel, work, |high| {
                    for (k, sample) in high.iter_mut().enumerate() {
                        let at = (frame + k / factor).min(frames - 1);
                        *sample = tube(model, offsets[at], *sample);
                    }
                    frame += high.len() / factor;
                });
            } else {
                for (sample, offset) in work.iter_mut().zip(offsets) {
                    *sample = tube(model, *offset, *sample);
                }
            }
        }

        // 3. Tone's inverse and Output.
        for channel in 0..channels {
            let out = audio.channel_mut(channel);
            for (i, sample) in out.iter_mut().enumerate() {
                *sample = self.tone_after[channel].process(self.work[channel][i]) * output[i];
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
        out[0] = if self.peak > 1e-6 {
            20.0 * self.peak.log10()
        } else {
            LEVEL_FLOOR_DB
        };
        self.peak = 0.0;
    }
}

/// The device's display (the tube distortion spec, R13): the tube's curve
/// over inputs from -1 to 1 at the Model, Drive and Bias now (with no
/// envelope), and, once an engine publishes, the input's level as a reading
/// and a marker on the curve.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0) as f32;
    let model = (value(MODEL) as usize).min(MODELS.len() - 1);
    let gain = db_to_gain(f64::from(value(DRIVE))) as f32;
    let offset = operating_point(model, value(BIAS) / 100.0);
    let through = |x: f32| tube(model, offset, gain * x);
    let mut out = DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: data.is_some(),
        view: None,
        series: vec![DisplaySeries {
            name: "Tube Curve".to_owned(),
            unit: String::new(),
            x_unit: String::new(),
            x_range: [-1.0, 1.0],
            log_x: false,
            y_range: [-1.0, 1.0],
            detail: false,
            values: (0..CURVE_POINTS)
                .map(|i| through(-1.0 + 2.0 * i as f32 / (CURVE_POINTS - 1) as f32))
                .collect(),
        }],
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
            let x = (db_to_gain(f64::from(input)) as f32).min(1.0);
            out.markers.push(DisplayMarker {
                name: "Level".to_owned(),
                series: "Tube Curve".to_owned(),
                x,
                y: through(x),
                text: format!("Level {input:.1} dB"),
            });
        }
    }
    out
}
