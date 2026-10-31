//! Color Limiter, the native limiter with character (WP-8.50; the Color
//! Limiter spec): Loudness drives the signal through a soft clipper
//! (Saturation, at twice the rate) and a tilt about 1 kHz (Color), and
//! then into [the Limiter](crate::limiter)'s look-ahead stage, which holds
//! the output under Ceiling.
//!
//! The ranges, defaults and both laws are Beatlayer's own reading of
//! Live's Creative Extensions pack page, which names the controls but
//! gives neither; see `docs/specs/color-limiter.md`.

use core::f64::consts::PI;

use bl_dsp::Oversampler;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayReading,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::limiter::{Brickwall, Settings};

/// Color Limiter's device type.
pub const COLOR_LIMITER: &str = "beatlayer.color_limiter";

/// Loudness: the gain into the device, up to +24 dB.
pub const LOUDNESS: ParamId = ParamId(0);
/// Ceiling: the highest level the output reaches, in dB.
pub const CEILING: ParamId = ParamId(1);
/// Lookahead: 1, 3 or 6 milliseconds.
pub const LOOKAHEAD: ParamId = ParamId(2);
/// Release: how quickly the gain recovers, in milliseconds.
pub const RELEASE: ParamId = ParamId(3);
/// Saturation: how hard the soft clipper bends, per cent.
pub const SATURATION: ParamId = ParamId(4);
/// Color: the tilt about 1 kHz, per cent.
pub const COLOR: ParamId = ParamId(5);

/// The look-ahead times the chooser offers, in seconds, and the one a
/// device without a saved value starts at.
const LOOKAHEAD_SECONDS: [f64; 3] = [0.001, 0.003, 0.006];
const DEFAULT_LOOKAHEAD: usize = 1;

/// Saturation's clipper runs at 2 times the rate.
const SATURATION_STAGES: u32 = 1;

/// The soft clipper's steepest `k`, at Saturation 100 %: `k` runs from 1.
const MAX_K: f32 = 4.0;

/// Color's tilt: the dB each end of the band moves at Color ±100 %, and
/// the frequency the tilt turns about.
const TILT_DB: f64 = 6.0;
const TILT_HZ: f64 = 1_000.0;

/// Frames between the tilt's coefficient updates, counted from the device's
/// first frame so a render doesn't depend on its block size.
const REFRESH_FRAMES: u64 = 16;

/// The lowest level the display reads, in dB.
const FLOOR_DB: f32 = -120.0;

/// The Color Limiter device type.
#[must_use]
pub fn color_limiter_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::gain(LOUDNESS, "Loudness", 24.0, 0.0)
            .describe("Drives the signal into the limiter, down to silence."),
        ParamDescriptor::continuous(CEILING, "Ceiling", (-24.0, 0.0), -0.3, ParamUnit::Decibels)
            .describe("The highest level the output reaches."),
        ParamDescriptor::choice(LOOKAHEAD, "Lookahead", &["1 ms", "3 ms", "6 ms"], 1)
            .describe("How far ahead the limiter sees a peak; it delays the output as long."),
        ParamDescriptor::continuous(
            RELEASE,
            "Release",
            (1.0, 1000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How quickly the gain recovers once a peak has passed."),
        ParamDescriptor::continuous(
            SATURATION,
            "Saturation",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How hard a soft clipper bends the signal before the limiter."),
        ParamDescriptor::continuous(COLOR, "Color", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe(
                "Tilts the tone about 1 kHz before the limiter: brighter above 0, darker \
                 below, so the highs are limited harder when brighter.",
            ),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(LOUDNESS, Widget::Knob)],
        )),
        LayoutNode::Cell(
            DisplayCell::new("Gain Reduction", 150.0, BODY_HEIGHT, 1.0)
                .with_bottom(ControlBand::stacked(60.0, &[(CEILING, Widget::Knob)])),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (SATURATION, Widget::Knob),
                (COLOR, Widget::Knob),
                (RELEASE, Widget::Knob),
                (LOOKAHEAD, Widget::Chooser),
            ],
        )),
    ]));
    DeviceType::native(COLOR_LIMITER, "Color Limiter", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[(
                "Color Limiter",
                &[
                    "Loudness",
                    "Ceiling",
                    "Lookahead",
                    "Release",
                    "Saturation",
                    "Color",
                ],
            )]),
    )
}

/// Adds Color Limiter to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, color_limiter_type(), make);
}

/// Makes a Color Limiter for `device`'s values and `config`'s rate and
/// block size. Its look-ahead comes from the set's value, so it reports its
/// latency before it first runs.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> ColorLimiter {
    let choice = device
        .params
        .get(&LOOKAHEAD)
        .copied()
        .unwrap_or(DEFAULT_LOOKAHEAD as f64);
    let frames = config.max_frames.max(1);
    ColorLimiter {
        rate: config.sample_rate.as_f64(),
        limiter: Brickwall::with_times(LOOKAHEAD_SECONDS, choice.max(0.0) as usize, config),
        oversampler: Oversampler::new(SATURATION_STAGES, 2, frames),
        tilt: [Tilt::default(); 2],
        ticks: 0,
    }
}

/// The soft clipper at Saturation `per_cent`: `tanh(k x) / k`, `k` from 1
/// to [`MAX_K`].
fn clip(x: f32, per_cent: f32) -> f32 {
    let k = 1.0 + (MAX_K - 1.0) * 0.01 * per_cent;
    (k * x).tanh() / k
}

/// The tilt: one first-order section with a zero at 1 kHz / G and a pole
/// at 1 kHz × G, so it passes 1/G at the bottom of the band, G at the top
/// and 1 at 1 kHz, G being [`TILT_DB`] at Color ±100 %.
#[derive(Clone, Copy)]
struct Tilt {
    b0: f64,
    b1: f64,
    a1: f64,
    z: f64,
}

impl Default for Tilt {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            a1: 0.0,
            z: 0.0,
        }
    }
}

impl Tilt {
    /// Designs the tilt for Color `per_cent` by the bilinear transform,
    /// which keeps the 1 kHz point exact.
    fn set(&mut self, per_cent: f32, rate: f64) {
        let g = 10f64.powf(f64::from(per_cent) * 0.01 * TILT_DB / 20.0);
        let k = (PI * TILT_HZ.min(rate * 0.45) / rate).tan().recip();
        let a0 = k + g;
        self.b0 = (g * k + 1.0) / a0;
        self.b1 = (1.0 - g * k) / a0;
        self.a1 = (g - k) / a0;
    }

    fn process(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let y = self.b0 * x + self.z;
        self.z = self.b1 * x - self.a1 * y;
        y as f32
    }
}

/// Color Limiter's audio-thread half.
pub struct ColorLimiter {
    rate: f64,
    /// The Limiter's look-ahead stage, called rather than copied.
    limiter: Brickwall,
    oversampler: Oversampler,
    tilt: [Tilt; 2],
    /// Frames processed so far, which the tilt's refresh counts.
    ticks: u64,
}

impl NativeDevice for ColorLimiter {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let index = |id: ParamId| id.0 as usize;
        let (loudness, saturation, color) = (
            params.ramp(index(LOUDNESS)),
            params.ramp(index(SATURATION)),
            params.ramp(index(COLOR)),
        );
        let factor = self.oversampler.factor();

        // 1. Loudness, Saturation at twice the rate, then Color's tilt.
        for channel in 0..channels {
            let samples = audio.channel_mut(channel);
            for (sample, gain) in samples.iter_mut().zip(loudness) {
                *sample *= gain;
            }
            let mut frame = 0;
            self.oversampler.process(channel, samples, |high| {
                for (k, sample) in high.iter_mut().enumerate() {
                    let at = (frame + k / factor).min(frames - 1);
                    *sample = clip(*sample, saturation[at]);
                }
                frame += high.len() / factor;
            });
            let tilt = &mut self.tilt[channel];
            for (i, sample) in samples.iter_mut().enumerate() {
                if (self.ticks + i as u64).is_multiple_of(REFRESH_FRAMES) {
                    tilt.set(color[i], self.rate);
                }
                *sample = tilt.process(*sample);
            }
        }
        self.ticks += frames as u64;

        // 2. The Limiter's look-ahead stage, under Ceiling.
        let value = |id: ParamId| params.value(index(id));
        self.limiter.limit(
            audio,
            &Settings {
                ceiling_db: value(CEILING),
                gain: None,
                release_ms: value(RELEASE),
                auto: false,
                lookahead: value(LOOKAHEAD) as usize,
                mode: 0,
                routing: 0,
                link: 1.0,
            },
        );
    }

    /// The look-ahead and the clipper's oversampler.
    fn latency(&self) -> u32 {
        self.limiter.latency() + self.oversampler.latency()
    }

    fn gain_reduction(&self) -> f32 {
        self.limiter.gain_reduction()
    }

    fn display_len(&self) -> usize {
        1
    }

    fn display(&mut self, out: &mut [f32]) {
        out[0] = self.limiter.gain_reduction().max(FLOOR_DB);
    }
}

/// The device's display (the Color Limiter spec, R1): the gain reduction
/// now, once an engine publishes. `data` is what the node published.
#[must_use]
pub fn display(device: DeviceId, data: Option<&[f32]>) -> DeviceDisplay {
    let readings = match data {
        Some([reduction]) => vec![DisplayReading {
            name: "Gain Reduction".to_owned(),
            value: *reduction,
            unit: "dB".to_owned(),
            text: format!("{reduction:.1} dB"),
        }],
        _ => Vec::new(),
    };
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings,
    }
}
