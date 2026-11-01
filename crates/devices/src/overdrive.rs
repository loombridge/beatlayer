//! Overdrive, the native overdrive (WP-5.23; the overdrive spec): a band-pass
//! before a `tanh` distortion, a Tone control after it, and Dynamics, which
//! levels what the distortion would otherwise make louder as Drive rises.
//!
//! The signal goes through the band-pass, a Drive gain of 6 to 36 dB (so
//! even Drive 0% distorts), the `tanh`, a level follower's gain (Dynamics),
//! and a high shelf (Tone), and is mixed with the dry signal. The curve,
//! the follower's times and the ranges are Beatlayer's own: the manual
//! names the controls but gives no numbers.

use core::f64::consts::{LN_2, TAU};

use bl_dsp::EnvelopeFollower;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::db_to_gain;
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker,
    DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::eq::{Type, coeffs};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::sections::Biquad;

/// Overdrive's device type.
pub const OVERDRIVE: &str = "beatlayer.grit";

/// Drive, per cent: the distortion's amount.
pub const DRIVE: ParamId = ParamId(0);
/// Tone, per cent: dark at 0, bright at 100, flat at 50.
pub const TONE: ParamId = ParamId(1);
/// Frequency: the band-pass's centre, in Hz.
pub const FREQUENCY: ParamId = ParamId(2);
/// Width: the band-pass's width between its −3 dB points, in octaves.
pub const WIDTH: ParamId = ParamId(3);
/// Dynamics, per cent: 0 levels the output as Drive rises, 100 doesn't.
pub const DYNAMICS: ParamId = ParamId(4);
/// Dry/Wet, per cent.
pub const DRY_WET: ParamId = ParamId(5);

/// The Drive gain at 0 % and 100 %, in dB.
const DRIVE_FROM_DB: f64 = 6.0;
const DRIVE_TO_DB: f64 = 36.0;
/// Tone: the high shelf's corner, and its gain at 0 % and 100 % (the
/// negative of it and itself), in dB.
const TONE_HZ: f64 = 1_500.0;
const TONE_RANGE_DB: f64 = 12.0;
/// Dynamics: how much of the level error the follower's gain corrects at
/// 0 % (as the exponent of the ratio), the level it corrects toward (the
/// distorted signal's mean absolute value), the gain's limits, and the
/// follower's times.
const LEVELLING: f32 = 0.8;
const TARGET_LEVEL: f32 = 0.3;
const LEVEL_FLOOR: f32 = 0.01;
const GAIN_LIMITS: (f32, f32) = (0.125, 8.0);
const ATTACK_SECONDS: f64 = 0.02;
const RELEASE_SECONDS: f64 = 0.2;
/// Frames between coefficient updates, counted from the device's first
/// frame, so a render doesn't depend on its block size.
const REFRESH_FRAMES: u64 = 16;

/// The display's curve: points across inputs from -1 to 1.
pub const CURVE_POINTS: usize = 97;
const LEVEL_FLOOR_DB: f32 = -120.0;

/// The distortion: a `tanh` of `gain` times the input.
fn curve(gain: f32, x: f32) -> f32 {
    (gain * x).tanh()
}

/// The Drive gain at `drive` per cent.
fn drive_gain(drive: f32) -> f32 {
    db_to_gain(DRIVE_FROM_DB + (DRIVE_TO_DB - DRIVE_FROM_DB) * f64::from(drive) / 100.0) as f32
}

/// The Overdrive device type.
#[must_use]
pub fn overdrive_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::continuous(DRIVE, "Drive", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How much the signal is distorted; even 0% distorts."),
        ParamDescriptor::continuous(TONE, "Tone", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("Shapes the highs after the distortion: darker below 50%, brighter above."),
        ParamDescriptor::continuous(
            FREQUENCY,
            "Frequency",
            (80.0, 8_000.0),
            1_200.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The centre of the band that reaches the distortion."),
        ParamDescriptor::continuous(WIDTH, "Width", (0.2, 4.0), 1.5, ParamUnit::None)
            .describe("The width of the band that reaches the distortion, in octaves."),
        ParamDescriptor::continuous(DYNAMICS, "Dynamics", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe(
                "How much the distortion's level follows Drive: at low settings more Drive \
                 means more compression and make-up gain, at high settings less.",
            ),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the dry signal and the distorted one."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Cell(
            DisplayCell::new("Drive Curve", 150.0, BODY_HEIGHT, 1.0).with_bottom(
                ControlBand::stacked(60.0, &[(FREQUENCY, Widget::Knob), (WIDTH, Widget::Knob)]),
            ),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (DRIVE, Widget::Knob),
                (TONE, Widget::Knob),
                (DYNAMICS, Widget::Knob),
                (DRY_WET, Widget::Knob),
            ],
        )),
    ]));
    DeviceType::native(OVERDRIVE, "Overdrive", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[(
                "Grit",
                &["Drive", "Tone", "Frequency", "Width", "Dynamics", "Dry/Wet"],
            )]),
    )
}

/// Adds Overdrive to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, overdrive_type(), make);
}

/// Makes an Overdrive for `config`'s rate. Every buffer is made here, off the
/// audio thread.
#[must_use]
pub fn make(_device: &Device, config: &NodeConfig) -> Grit {
    Grit::new(config)
}

/// A band-pass of constant 0 dB peak gain (the audio-EQ cookbook's), with
/// its width in octaves between the −3 dB points.
#[derive(Clone, Copy, Default)]
struct BandPass {
    b0: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl BandPass {
    fn design(&mut self, freq: f64, octaves: f64, rate: f64) {
        let w0 = TAU * (freq / rate).clamp(1.0e-6, 0.45);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin * (LN_2 / 2.0 * octaves * w0 / sin).sinh();
        let a0 = 1.0 + alpha;
        self.b0 = (alpha / a0) as f32;
        self.a1 = (-2.0 * cos / a0) as f32;
        self.a2 = ((1.0 - alpha) / a0) as f32;
    }

    /// Transposed direct form II with b1 = 0 and b2 = −b0.
    fn process(&mut self, x: f32) -> f32 {
        let flush = |v: f32| if v.abs() < 1e-30 { 0.0 } else { v };
        let y = flush(self.b0 * x + self.z1);
        self.z1 = flush(self.z2 - self.a1 * y);
        self.z2 = flush(-self.b0 * x - self.a2 * y);
        y
    }
}

/// Overdrive's audio-thread half.
pub struct Grit {
    rate: f64,
    band: [BandPass; 2],
    tone: [Biquad; 2],
    followers: [EnvelopeFollower; 2],
    /// Frames processed so far, which the coefficient refresh counts.
    ticks: u64,
    /// The input's peak since the last block, for the display.
    peak: f32,
}

impl Grit {
    fn new(config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let mut followers = [EnvelopeFollower::new(rate), EnvelopeFollower::new(rate)];
        for follower in &mut followers {
            follower.set(ATTACK_SECONDS, RELEASE_SECONDS, false);
        }
        // A shelf that does nothing until the first refresh sets it.
        let mut flat = Biquad::default();
        flat.c = coeffs(Type::HighShelf, TONE_HZ, 0.0, 0.71, rate);
        let mut band = BandPass::default();
        band.design(1_200.0, 1.5, rate);
        Self {
            rate,
            band: [band; 2],
            tone: [flat; 2],
            followers,
            ticks: 0,
            peak: 0.0,
        }
    }
}

impl NativeDevice for Grit {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let ramp = |id: ParamId| params.ramp(id.0 as usize);
        let (drive, tone, frequency, width, dynamics, mix) = (
            ramp(DRIVE),
            ramp(TONE),
            ramp(FREQUENCY),
            ramp(WIDTH),
            ramp(DYNAMICS),
            ramp(DRY_WET),
        );
        for i in 0..frames {
            if (self.ticks + i as u64).is_multiple_of(REFRESH_FRAMES) {
                for channel in 0..channels {
                    self.band[channel].design(
                        f64::from(frequency[i]),
                        f64::from(width[i]),
                        self.rate,
                    );
                    self.tone[channel].c = coeffs(
                        Type::HighShelf,
                        TONE_HZ,
                        TONE_RANGE_DB * (f64::from(tone[i]) - 50.0) / 50.0,
                        0.71,
                        self.rate,
                    );
                }
            }
            let gain = drive_gain(drive[i]);
            let mut shaped = [0.0f32; 2];
            let mut level = 0.0f32;
            for (channel, out) in shaped.iter_mut().enumerate().take(channels) {
                let x = audio.channel_mut(channel)[i];
                self.peak = self.peak.max(x.abs());
                *out = curve(gain, self.band[channel].process(x));
                level = level.max(self.followers[channel].process(*out) as f32);
            }
            // Dynamics: pull the level toward the target, as far as the
            // setting allows.
            let leveling = LEVELLING * (1.0 - dynamics[i] / 100.0);
            let correction = (TARGET_LEVEL / level.max(LEVEL_FLOOR))
                .powf(leveling)
                .clamp(GAIN_LIMITS.0, GAIN_LIMITS.1);
            for (channel, shaped) in shaped.iter().enumerate().take(channels) {
                let wet = self.tone[channel].process(shaped * correction);
                let sample = &mut audio.channel_mut(channel)[i];
                *sample += (wet - *sample) * (mix[i] / 100.0);
            }
        }
        self.ticks += frames as u64;
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

/// The device's display (the overdrive spec, R10): the distortion curve
/// over inputs from -1 to 1 at the Drive now, and, once an engine
/// publishes, the input's level as a reading and a marker on the curve.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let gain = drive_gain(description.value(values, DRIVE).unwrap_or(0.0) as f32);
    let mut out = DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: data.is_some(),
        view: None,
        series: vec![DisplaySeries {
            name: "Drive Curve".to_owned(),
            unit: String::new(),
            x_unit: String::new(),
            x_range: [-1.0, 1.0],
            log_x: false,
            y_range: [-1.0, 1.0],
            detail: false,
            values: (0..CURVE_POINTS)
                .map(|i| curve(gain, -1.0 + 2.0 * i as f32 / (CURVE_POINTS - 1) as f32))
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
                handle: None,
                name: "Level".to_owned(),
                series: "Drive Curve".to_owned(),
                x,
                y: curve(gain, x),
                text: format!("Level {input:.1} dB"),
            });
        }
    }
    out
}
