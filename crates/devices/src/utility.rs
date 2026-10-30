//! The Utility device (WP-2.18b): gain, channel modes, width, mono, bass
//! mono, phase invert per side, balance, the DC filter and mute, matching
//! Live 12's Utility (the utility spec).
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use core::f32::consts::TAU;
use core::mem;

use bl_engine::{DeviceType, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// The Utility device type's id.
pub const UTILITY: &str = "beatlayer.utility";

/// The left channel's phase invert.
pub const PHASE_LEFT: ParamId = ParamId(0);
/// The right channel's phase invert.
pub const PHASE_RIGHT: ParamId = ParamId(1);
/// Which input channels reach the outputs.
pub const CHANNEL_MODE: ParamId = ParamId(2);
/// Converts the input to mono.
pub const MONO: ParamId = ParamId(3);
/// Makes the low side signal mono.
pub const BASS_MONO: ParamId = ParamId(4);
/// The frequency below which Bass Mono removes the side signal, in Hz.
pub const BASS_MONO_FREQUENCY: ParamId = ParamId(5);
/// The stereo width, in per cent.
pub const WIDTH: ParamId = ParamId(6);
/// How far the signal is panned, from left to right.
pub const BALANCE: ParamId = ParamId(7);
/// Filters out DC and frequencies far below the audible range.
pub const DC: ParamId = ParamId(8);
/// The gain.
pub const GAIN: ParamId = ParamId(9);
/// Silences the input.
pub const MUTE: ParamId = ParamId(10);

/// The Channel Mode choices: both channels, then each alone, then a swap.
pub const CHANNEL_MODES: [&str; 4] = ["Stereo", "Left", "Right", "Swap"];

/// The DC filter's cutoff, in Hz: far below the audible range.
const DC_CUTOFF_HZ: f32 = 5.0;

/// The Utility device's type.
#[must_use]
pub fn utility_type() -> DeviceType {
    DeviceType::native(UTILITY, "Utility", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::toggle(PHASE_LEFT, "Phase Left", false)
                .describe("Inverts the phase of the left channel."),
            ParamDescriptor::toggle(PHASE_RIGHT, "Phase Right", false)
                .describe("Inverts the phase of the right channel."),
            ParamDescriptor::choice(CHANNEL_MODE, "Channel Mode", &CHANNEL_MODES, 0)
                .describe("Processes both channels, or only the left or right one, or swaps them."),
            ParamDescriptor::toggle(MONO, "Mono", false)
                .describe("Converts the stereo input to mono."),
            ParamDescriptor::toggle(BASS_MONO, "Bass Mono", false)
                .describe("Makes the side signal below Bass Mono Frequency mono."),
            ParamDescriptor::continuous(
                BASS_MONO_FREQUENCY,
                "Bass Mono Frequency",
                (50.0, 500.0),
                120.0,
                ParamUnit::Hertz,
            )
            .logarithmic()
            .describe("The frequency below which Bass Mono removes the side signal."),
            ParamDescriptor::continuous(WIDTH, "Width", (0.0, 200.0), 100.0, ParamUnit::Percent)
                .describe("0 % is mono, 100 % is unchanged, 200 % doubles the side signal."),
            ParamDescriptor::continuous(BALANCE, "Balance", (-50.0, 50.0), 0.0, ParamUnit::None)
                .decimals(0)
                .describe("Pans the signal in the stereo field."),
            ParamDescriptor::toggle(DC, "DC", false)
                .describe("Filters out DC offsets and frequencies far below the audible range."),
            ParamDescriptor::gain(GAIN, "Gain", 35.0, 0.0)
                .describe("Scales the signal, down to silence."),
            ParamDescriptor::toggle(MUTE, "Mute", false).describe("Silences the input."),
        ])
        .with_banks(&[
            (
                "Utility",
                &[
                    "Gain",
                    "Width",
                    "Balance",
                    "Mute",
                    "Mono",
                    "Phase Left",
                    "Phase Right",
                    "Channel Mode",
                ],
            ),
            ("Bass", &["Bass Mono", "Bass Mono Frequency", "DC"]),
        ]),
    )
}

/// Adds the Utility device to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, utility_type(), |_, _| Utility::new());
}

/// Utility's state between blocks: the DC filter's state per channel and the
/// side signal's low-pass, for Bass Mono.
struct Utility {
    /// Each channel's DC filter: its last input and its last output.
    dc: [(f32, f32); 2],
    /// The side signal's low-pass, for Bass Mono.
    bass: f32,
}

impl Utility {
    fn new() -> Self {
        Self {
            dc: [(0.0, 0.0); 2],
            bass: 0.0,
        }
    }

    /// One frame of channel `channel`'s DC filter, with coefficient `a`.
    fn dc_filter(&mut self, channel: usize, x: f32, a: f32) -> f32 {
        let (last_in, last_out) = self.dc[channel];
        let y = flush(a * (last_out + x - last_in));
        self.dc[channel] = (x, y);
        y
    }
}

/// A value far below the audio's noise floor is left as zero rather than as
/// a denormal, whose arithmetic is slow.
fn flush(x: f32) -> f32 {
    if x.abs() < 1e-30 { 0.0 } else { x }
}

impl NativeDevice for Utility {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels();
        if frames == 0 || channels == 0 {
            return;
        }
        // Where each parameter sits in the bank, which the description
        // always holds.
        let at = |id| params.index(id).expect("the description holds it");
        // A device in a chain is stereo; a single-channel block is treated
        // as a left channel alone.
        let stereo = channels >= 2;
        let mode = if stereo {
            params.value(at(CHANNEL_MODE)) as usize
        } else {
            0
        };
        let phase = [
            params.value(at(PHASE_LEFT)) >= 0.5,
            params.value(at(PHASE_RIGHT)) >= 0.5,
        ];
        let (mono, bass_mono, dc, mute) = (
            params.value(at(MONO)) >= 0.5,
            params.value(at(BASS_MONO)) >= 0.5,
            params.value(at(DC)) >= 0.5,
            params.value(at(MUTE)) >= 0.5,
        );
        let (width, balance, gain, bass_hz) = (
            params.ramp(at(WIDTH)),
            params.ramp(at(BALANCE)),
            params.ramp(at(GAIN)),
            params.ramp(at(BASS_MONO_FREQUENCY)),
        );
        let rate = audio.sample_rate().as_f64() as f32;
        let a = 1.0 / (1.0 + TAU * DC_CUTOFF_HZ / rate);
        for i in 0..frames {
            let input = audio.channel_mut(0)[i];
            let mut left = input;
            let mut right = if stereo {
                audio.channel_mut(1)[i]
            } else {
                input
            };
            if dc {
                left = self.dc_filter(0, left, a);
                right = self.dc_filter(1, right, a);
            }
            if phase[0] {
                left = -left;
            }
            if phase[1] {
                right = -right;
            }
            match mode {
                // Left and Right put one channel on both outputs; Swap
                // exchanges them. Width and Bass Mono don't apply.
                1 => right = left,
                2 => left = right,
                3 => mem::swap(&mut left, &mut right),
                _ => {
                    let mid = 0.5 * (left + right);
                    let mut side = 0.5 * (left - right) * width[i] * 0.01;
                    let alpha = 1.0 - (-TAU * bass_hz[i] / rate).exp();
                    self.bass = flush(self.bass + alpha * (side - self.bass));
                    if bass_mono {
                        side -= self.bass;
                    }
                    left = mid + side;
                    right = mid - side;
                }
            }
            if mono {
                let m = 0.5 * (left + right);
                left = m;
                right = m;
            }
            let b = (balance[i] / 50.0).clamp(-1.0, 1.0);
            let g = gain[i];
            left *= g * (1.0 - b.max(0.0));
            right *= g * (1.0 + b.min(0.0));
            if mute {
                left = 0.0;
                right = 0.0;
            }
            audio.channel_mut(0)[i] = left;
            if stereo {
                audio.channel_mut(1)[i] = right;
            }
        }
    }
}
