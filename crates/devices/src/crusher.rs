//! Crush, the native bit-crusher (WP-5.24; the bit-crusher spec): lowers the
//! sample rate and the bit depth. The signal goes through an optional
//! low-pass, is held at the lowered rate, goes through an optional low-pass
//! again, and is then quantised.
//!
//! The clock's jitter follows the device's seed, so a render repeats run
//! after run, and every value is worked out frame by frame, so it is the
//! same at any block size.

use bl_dsp::Random;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::eq::{Type, coeffs};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::sections::{Biquad, Coeffs};

/// Crush's device type.
pub const CRUSHER: &str = "beatlayer.crush";

/// Rate: the sample rate the signal is held at.
pub const RATE: ParamId = ParamId(0);
/// Jitter: noise on the clock that holds the signal.
pub const JITTER: ParamId = ParamId(1);
/// Pre: a low-pass before the hold, at half the Rate.
pub const PRE: ParamId = ParamId(2);
/// Post: a low-pass after the hold.
pub const POST: ParamId = ParamId(3);
/// Post Octave: the Post filter's corner, in octaves from half the Rate.
pub const POST_OCTAVE: ParamId = ParamId(4);
/// Bits: how many bits the output has.
pub const BITS: ParamId = ParamId(5);
/// Shape: finer steps for quiet parts.
pub const SHAPE: ParamId = ParamId(6);
/// DC Shift: an offset before the quantiser, in per cent of one step.
pub const DC_SHIFT: ParamId = ParamId(7);
/// Dry/Wet.
pub const DRY_WET: ParamId = ParamId(8);

const RATE_INDEX: usize = 0;
const JITTER_INDEX: usize = 1;
const PRE_INDEX: usize = 2;
const POST_INDEX: usize = 3;
const POST_OCTAVE_INDEX: usize = 4;
const BITS_INDEX: usize = 5;
const SHAPE_INDEX: usize = 6;
const DC_INDEX: usize = 7;
const WET_INDEX: usize = 8;

/// The Qs of two second-order stages that make a 4th-order Butterworth.
const BUTTERWORTH_Q: [f64; 2] = [0.5412, 1.3066];
/// The highest corner a filter has, as a fraction of the sample rate.
const MAX_CORNER: f64 = 0.45;
/// At Jitter 100 %, the clock's step varies by up to this fraction.
const JITTER_SPREAD: f64 = 0.5;
/// At Shape 100 %, the quantiser works on the signal raised to the power
/// `1 / (1 + SHAPE_RANGE)`.
const SHAPE_RANGE: f64 = 3.0;
/// How often the filters' coefficients are worked out, in frames.
const REFRESH_FRAMES: u64 = 16;

/// The Crush device type.
#[must_use]
pub fn crusher_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::continuous(RATE, "Rate", (100.0, 48_000.0), 8_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The sample rate the signal is reduced to. At the engine's rate or above, it is untouched."),
        ParamDescriptor::continuous(JITTER, "Jitter", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Noise on the clock of the rate reduction. Each channel has its own, which widens the stereo image."),
        ParamDescriptor::toggle(PRE, "Pre", false)
            .describe("Low-passes the signal before the rate reduction, at half the Rate."),
        ParamDescriptor::toggle(POST, "Post", false)
            .describe("Low-passes the signal after the rate reduction, to soften its images."),
        ParamDescriptor::continuous(
            POST_OCTAVE,
            "Post Octave",
            (-4.0, 4.0),
            0.0,
            ParamUnit::None,
        )
        .describe("The Post filter's corner, in octaves above or below half the Rate."),
        ParamDescriptor::integer(BITS, "Bits", (2, 16), 8, ParamUnit::None)
            .describe("How many bits the output is quantised to."),
        ParamDescriptor::continuous(SHAPE, "Shape", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Makes the quantiser's steps finer at small amplitudes."),
        ParamDescriptor::continuous(DC_SHIFT, "DC Shift", (-50.0, 50.0), 0.0, ParamUnit::Percent)
            .describe("Adds an offset, in per cent of one step, before the quantiser."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the dry and the crushed signal."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (RATE, Widget::Knob),
                (JITTER, Widget::Knob),
                (PRE, Widget::Toggle),
                (POST, Widget::Toggle),
                (POST_OCTAVE, Widget::Knob),
            ],
        )),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (BITS, Widget::Number),
                (SHAPE, Widget::Knob),
                (DC_SHIFT, Widget::Knob),
                (DRY_WET, Widget::Knob),
            ],
        )),
    ]));
    DeviceType::native(CRUSHER, "Crush", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout))
}

/// Adds Crush to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, crusher_type(), Crusher::new);
}

/// `x` quantised to `bits` bits: a mid-tread quantiser of `2^bits` levels
/// from −1 up to one step under 1. `power` below 1 (Shape) quantises the
/// signal raised to it, so steps near zero are finer; `offset` (DC Shift) is
/// in steps and is added before the rounding.
fn quantise(x: f32, bits: u32, power: f32, offset: f32) -> f32 {
    let half = (1u32 << (bits - 1)) as f32;
    let x = x.clamp(-1.0, 1.0);
    let curved = if power == 1.0 {
        x
    } else {
        x.signum() * x.abs().powf(power)
    };
    let level = (curved * half + offset).round().clamp(-half, half - 1.0) / half;
    if power == 1.0 {
        return level;
    }
    // Back out of the curve, scaled so the top level stays where it was.
    let top = (half - 1.0) / half;
    let restored = top.powf(1.0 - 1.0 / power) * level.signum() * level.abs().powf(1.0 / power);
    restored.clamp(-1.0, 1.0)
}

/// A 4th-order low-pass at `corner` Hz: two stages.
fn low_pass(corner: f64, rate: f64) -> [Coeffs; 2] {
    let corner = corner.min(rate * MAX_CORNER);
    BUTTERWORTH_Q.map(|q| coeffs(Type::HighCut12, corner, 0.0, q, rate))
}

/// One channel's state.
struct Channel {
    /// The clock's phase, in held samples, and what it holds.
    phase: f64,
    held: f32,
    jitter: Random,
    pre: [Biquad; 2],
    post: [Biquad; 2],
}

/// Crush's audio-thread half.
pub struct Crusher {
    rate: f64,
    channels: Vec<Channel>,
    frames_run: u64,
}

impl Crusher {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let seed = device.id.0.get();
        Self {
            rate: config.sample_rate.as_f64(),
            channels: (0..config.channels)
                .map(|channel| Channel {
                    phase: 0.0,
                    held: 0.0,
                    jitter: Random::seeded(seed.wrapping_add(channel as u64)),
                    pre: [Biquad::default(); 2],
                    post: [Biquad::default(); 2],
                })
                .collect(),
            frames_run: 0,
        }
    }
}

impl NativeDevice for Crusher {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let (rate, first) = (self.rate, self.frames_run);
        let (hold, jitter, octave) = (
            params.ramp(RATE_INDEX),
            params.ramp(JITTER_INDEX),
            params.ramp(POST_OCTAVE_INDEX),
        );
        let (bits, shape, dc, wet) = (
            params.ramp(BITS_INDEX),
            params.ramp(SHAPE_INDEX),
            params.ramp(DC_INDEX),
            params.ramp(WET_INDEX),
        );
        let (pre_on, post_on) = (
            params.value(PRE_INDEX) >= 0.5,
            params.value(POST_INDEX) >= 0.5,
        );
        for (index, channel) in self.channels.iter_mut().enumerate() {
            if index >= audio.channels() {
                break;
            }
            for (i, sample) in audio.channel_mut(index).iter_mut().enumerate() {
                let dry = *sample;
                let step = (f64::from(hold[i]) / rate).min(1.0);
                if (first + i as u64).is_multiple_of(REFRESH_FRAMES) {
                    let half_rate = f64::from(hold[i]) / 2.0;
                    let post = half_rate * 2f64.powf(f64::from(octave[i]));
                    let (pre, post) = (low_pass(half_rate, rate), low_pass(post, rate));
                    for (stage, c) in channel.pre.iter_mut().zip(pre) {
                        stage.c = c;
                    }
                    for (stage, c) in channel.post.iter_mut().zip(post) {
                        stage.c = c;
                    }
                }
                let mut x = dry;
                if pre_on && step < 1.0 {
                    x = channel.pre.iter_mut().fold(x, |x, stage| stage.process(x));
                }
                let noise = channel.jitter.next_bipolar();
                channel.phase +=
                    step * (1.0 + JITTER_SPREAD * f64::from(jitter[i]) / 100.0 * noise);
                if channel.phase >= 1.0 {
                    channel.phase = (channel.phase - 1.0).min(1.0);
                    channel.held = x;
                }
                let mut y = channel.held;
                if post_on {
                    y = channel.post.iter_mut().fold(y, |y, stage| stage.process(y));
                }
                let power = 1.0 / (1.0 + SHAPE_RANGE * f64::from(shape[i]) / 100.0);
                let y = quantise(y, bits[i] as u32, power as f32, dc[i] / 100.0);
                *sample = dry + (y - dry) * (wet[i] / 100.0);
            }
        }
        self.frames_run += audio.frames() as u64;
    }
}
