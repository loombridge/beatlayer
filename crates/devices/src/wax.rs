//! Wax, the native vinyl-distortion effect (WP-5.24; the vinyl distortion
//! spec): two band-limited distortions, one adding even harmonics (the
//! tracing model) and one odd harmonics, in opposite phase between the
//! channels (the pinch), under one drive, and a crackle of random clicks.
//!
//! The clicks follow the device's seed, so a render repeats run after run,
//! and every value is worked out frame by frame, so it is the same at any
//! block size.

use bl_dsp::Random;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::degrade::q_for_octaves;
use crate::delay::BandPass;
use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Wax's device type.
pub const WAX: &str = "beatlayer.wax";

/// Tracing Drive: how much even-harmonic distortion the tracing model adds.
pub const TRACING_DRIVE: ParamId = ParamId(0);
/// Tracing Freq: the centre of the band the tracing model distorts.
pub const TRACING_FREQ: ParamId = ParamId(1);
/// Tracing Width: that band's width, in octaves.
pub const TRACING_WIDTH: ParamId = ParamId(2);
/// Pinch Drive: how much odd-harmonic distortion the pinch adds.
pub const PINCH_DRIVE: ParamId = ParamId(3);
/// Pinch Freq: the centre of the band the pinch distorts.
pub const PINCH_FREQ: ParamId = ParamId(4);
/// Pinch Width: that band's width, in octaves.
pub const PINCH_WIDTH: ParamId = ParamId(5);
/// Drive: the overall distortion, from both models.
pub const DRIVE: ParamId = ParamId(6);
/// Soft or Hard: a dub plate or a standard record.
pub const SOFT_HARD: ParamId = ParamId(7);
/// Pinch Stereo or Mono.
pub const PINCH_MODE: ParamId = ParamId(8);
/// Crackle Density: clicks per second.
pub const CRACKLE_DENSITY: ParamId = ParamId(9);
/// Crackle Volume.
pub const CRACKLE_VOLUME: ParamId = ParamId(10);

const TRACING_DRIVE_INDEX: usize = 0;
const TRACING_FREQ_INDEX: usize = 1;
const TRACING_WIDTH_INDEX: usize = 2;
const PINCH_DRIVE_INDEX: usize = 3;
const PINCH_FREQ_INDEX: usize = 4;
const PINCH_WIDTH_INDEX: usize = 5;
const DRIVE_INDEX: usize = 6;
const SOFT_HARD_INDEX: usize = 7;
const PINCH_MODE_INDEX: usize = 8;
const DENSITY_INDEX: usize = 9;
const VOLUME_INDEX: usize = 10;

/// The distortion added at full drive, against the band's amplitude: the
/// tracing model adds `TRACING_GAIN * band^2`, the pinch `PINCH_GAIN * band^3`.
const TRACING_GAIN: f32 = 2.0;
const PINCH_GAIN: f32 = 2.0;
/// Soft scales the distortion by this and low-passes it at [`SOFT_CORNER`].
const SOFT_SCALE: f32 = 0.5;
const SOFT_CORNER: f64 = 4_000.0;
/// The tracing model's squared band has a DC part: this high-pass removes it.
const DC_CORNER: f64 = 20.0;
/// The shortest gap between two clicks, in seconds, and how quickly a click
/// dies away: its time constant, in seconds.
const MIN_GAP_SECONDS: f64 = 0.002;
const CLICK_SECONDS: f64 = 0.0002;
/// A click's peak is a random value between this and 1, of random sign.
const CLICK_FLOOR: f64 = 0.25;
/// The most clicks a second Density allows: the shortest gap's reciprocal
/// is 500, so there is always room for the wait that makes the average.
const MAX_DENSITY: f64 = 250.0;
/// How often the bands' coefficients are worked out, in frames.
const REFRESH_FRAMES: u64 = 16;

/// The Wax device type.
#[must_use]
pub fn wax_type() -> DeviceType {
    let width = |id, name: &str| {
        ParamDescriptor::continuous(id, name, (0.25, 4.0), 1.0, ParamUnit::None)
            .logarithmic()
            .describe("The width of the band that is distorted, in octaves.")
    };
    let drive = |id, name: &str, text: &str| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), 50.0, ParamUnit::Percent).describe(text)
    };
    let freq = |id, name: &str, default| {
        ParamDescriptor::continuous(id, name, (20.0, 20_000.0), default, ParamUnit::Hertz)
            .logarithmic()
            .describe("The centre of the band that is distorted.")
    };
    let params = vec![
        drive(
            TRACING_DRIVE,
            "Tracing Drive",
            "How much even-harmonic distortion the tracing model adds.",
        ),
        freq(TRACING_FREQ, "Tracing Freq", 3_000.0),
        width(TRACING_WIDTH, "Tracing Width"),
        drive(
            PINCH_DRIVE,
            "Pinch Drive",
            "How much odd-harmonic distortion the pinch adds.",
        ),
        freq(PINCH_FREQ, "Pinch Freq", 6_000.0),
        width(PINCH_WIDTH, "Pinch Width"),
        ParamDescriptor::continuous(DRIVE, "Drive", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("Raises or lowers the distortion of both models together."),
        ParamDescriptor::choice(SOFT_HARD, "Soft/Hard", &["Soft", "Hard"], 1).describe(
            "Soft is a duller, gentler distortion, like a dub plate; Hard is a standard record.",
        ),
        ParamDescriptor::choice(PINCH_MODE, "Pinch Mode", &["Stereo", "Mono"], 0).describe(
            "Whether the pinch's distortion is opposite in the two channels or the same.",
        ),
        ParamDescriptor::continuous(
            CRACKLE_DENSITY,
            "Crackle Density",
            (0.0, MAX_DENSITY),
            0.0,
            ParamUnit::Hertz,
        )
        .describe("How many clicks of crackle there are each second."),
        ParamDescriptor::gain(CRACKLE_VOLUME, "Crackle Volume", 6.0, -24.0)
            .describe("How loud the crackle is, down to silence."),
    ];
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        band(&[
            (TRACING_DRIVE, Widget::Knob),
            (TRACING_FREQ, Widget::Knob),
            (TRACING_WIDTH, Widget::Knob),
        ]),
        band(&[
            (PINCH_DRIVE, Widget::Knob),
            (PINCH_FREQ, Widget::Knob),
            (PINCH_WIDTH, Widget::Knob),
            (PINCH_MODE, Widget::Chooser),
        ]),
        band(&[(DRIVE, Widget::Knob), (SOFT_HARD, Widget::Chooser)]),
        band(&[
            (CRACKLE_DENSITY, Widget::Knob),
            (CRACKLE_VOLUME, Widget::Knob),
        ]),
    ]));
    DeviceType::native(WAX, "Wax", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout))
}

/// Adds Wax to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, wax_type(), Wax::new);
}

/// One channel's state.
struct Channel {
    tracing: BandPass,
    pinch: BandPass,
    /// The high-pass that removes the tracing term's DC: the last input and
    /// output.
    dc: (f32, f32),
    /// The Soft mode's low-pass.
    soft: f32,
    clicks: Random,
    /// Frames since the last click, and its peak now.
    since: u64,
    click: f32,
}

/// Wax's audio-thread half.
pub struct Wax {
    rate: f64,
    channels: Vec<Channel>,
    frames_run: u64,
    /// The per-frame multiplier of a click's peak, the DC high-pass's pole
    /// and the Soft low-pass's step.
    decay: f32,
    dc_pole: f32,
    soft_step: f32,
    min_gap: u64,
}

impl Wax {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let seed = device.id.0.get();
        let one_pole = |corner: f64| (1.0 - (-std::f64::consts::TAU * corner / rate).exp()) as f32;
        Self {
            rate,
            channels: (0..config.channels)
                .map(|channel| Channel {
                    tracing: BandPass::default(),
                    pinch: BandPass::default(),
                    dc: (0.0, 0.0),
                    soft: 0.0,
                    clicks: Random::seeded(seed.wrapping_add(channel as u64)),
                    since: 0,
                    click: 0.0,
                })
                .collect(),
            frames_run: 0,
            decay: (-1.0 / (CLICK_SECONDS * rate)).exp() as f32,
            dc_pole: 1.0 - one_pole(DC_CORNER),
            soft_step: one_pole(SOFT_CORNER),
            min_gap: (MIN_GAP_SECONDS * rate).round() as u64,
        }
    }
}

impl NativeDevice for Wax {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let (rate, first) = (self.rate, self.frames_run);
        let ramp = |index| params.ramp(index);
        let (tracing_drive, pinch_drive, drive) = (
            ramp(TRACING_DRIVE_INDEX),
            ramp(PINCH_DRIVE_INDEX),
            ramp(DRIVE_INDEX),
        );
        let (tracing_freq, tracing_width) = (ramp(TRACING_FREQ_INDEX), ramp(TRACING_WIDTH_INDEX));
        let (pinch_freq, pinch_width) = (ramp(PINCH_FREQ_INDEX), ramp(PINCH_WIDTH_INDEX));
        let (density, volume) = (ramp(DENSITY_INDEX), ramp(VOLUME_INDEX));
        let soft = params.value(SOFT_HARD_INDEX) < 0.5;
        let mono = params.value(PINCH_MODE_INDEX) >= 0.5;
        let max_corner = rate * 0.45;
        for (index, channel) in self.channels.iter_mut().enumerate() {
            if index >= audio.channels() {
                break;
            }
            // Stereo: the pinch is added to one channel and taken from the
            // other.
            let sign = if index % 2 == 1 && !mono { -1.0 } else { 1.0 };
            for (i, sample) in audio.channel_mut(index).iter_mut().enumerate() {
                if (first + i as u64).is_multiple_of(REFRESH_FRAMES) {
                    let band = |freq: f32, width: f32| {
                        (
                            f64::from(freq).min(max_corner),
                            q_for_octaves(f64::from(width)),
                        )
                    };
                    let (f, q) = band(tracing_freq[i], tracing_width[i]);
                    channel.tracing.set(f, q, rate);
                    let (f, q) = band(pinch_freq[i], pinch_width[i]);
                    channel.pinch.set(f, q, rate);
                }
                let x = *sample;
                let overall = drive[i] / 100.0;
                let even = channel.tracing.process(x);
                let even = even * even;
                // The high-pass: out = in - last in + pole * last out.
                let blocked = even - channel.dc.0 + self.dc_pole * channel.dc.1;
                channel.dc = (even, blocked);
                let odd = channel.pinch.process(x);
                let mut added = TRACING_GAIN * tracing_drive[i] / 100.0 * overall * blocked
                    + sign * PINCH_GAIN * pinch_drive[i] / 100.0 * overall * odd.powi(3);
                if soft {
                    channel.soft += (added - channel.soft) * self.soft_step;
                    added = channel.soft * SOFT_SCALE;
                }

                // Crackle: after the shortest gap, each frame has a chance of
                // a click that makes the average gap 1 / Density.
                channel.since += 1;
                channel.click *= self.decay;
                let wait = 1.0 / f64::from(density[i]).max(1e-9) - MIN_GAP_SECONDS;
                if density[i] > 0.0
                    && channel.since >= self.min_gap
                    && (channel.clicks.next_bipolar() + 1.0) * 0.5 * wait * rate < 1.0
                {
                    let peak = CLICK_FLOOR
                        + (1.0 - CLICK_FLOOR) * (channel.clicks.next_bipolar() + 1.0) * 0.5;
                    let sign = if channel.clicks.next_bipolar() < 0.0 {
                        -1.0
                    } else {
                        1.0
                    };
                    channel.click = (peak * sign) as f32;
                    channel.since = 0;
                }
                *sample = x + added + channel.click * volume[i];
            }
        }
        self.frames_run += audio.frames() as u64;
    }
}
