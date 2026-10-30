//! The External Audio Effect (WP-5.35; the external devices spec, R17 to
//! R21): an audio effect that sends the track's signal out of the audio
//! interface to a hardware processor and brings it back, with a Gain each
//! way, Dry/Wet, Invert and a Hardware Latency.
//!
//! The engine does the routing: it maps the interface's inputs the device
//! is set to into its block, gives it a buffer to write the signal it
//! sends out into, and reports the round trip plus its Hardware Latency to
//! delay compensation, all from the settings in the document
//! ([`bl_model::External`]). The node sends its input out, delays its dry
//! signal by the device's latency so the dry and the returning signal line
//! up, and mixes the two.

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::{Device, DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// The External Audio Effect's type id.
pub const HARDWARE_EFFECT: &str = "beatlayer.hardware_effect";

/// The level of the signal sent to the hardware.
pub const GAIN_OUT: ParamId = ParamId(0);
/// The level of the signal coming back.
pub const GAIN_IN: ParamId = ParamId(1);
/// How much of the returning signal is heard, against the dry one.
pub const DRY_WET: ParamId = ParamId(2);
/// Whether the returning signal's phase is flipped.
pub const INVERT: ParamId = ParamId(3);

/// The External Audio Effect's type: an audio effect that works with
/// hardware, sending its signal out and hearing it back.
#[must_use]
pub fn hardware_effect_type() -> DeviceType {
    DeviceType::native(
        HARDWARE_EFFECT,
        "External Audio Effect",
        DeviceRole::AudioEffect,
    )
    .with_external_output()
    .with_description(
        DeviceDescription::new(vec![
            // Spec: external-devices R17, R21. -inf to +24 dB, 0 dB on a new
            // device.
            ParamDescriptor::gain(GAIN_OUT, "Out Gain", 24.0, 0.0)
                .describe("Scales the signal sent to the hardware."),
            ParamDescriptor::gain(GAIN_IN, "In Gain", 24.0, 0.0)
                .describe("Scales the signal coming back from the hardware."),
            ParamDescriptor::continuous(
                DRY_WET,
                "Dry/Wet",
                (0.0, 100.0),
                100.0,
                ParamUnit::Percent,
            )
            .describe("Balances the dry signal against the processed one."),
            ParamDescriptor::toggle(INVERT, "Invert", false)
                .describe("Flips the phase of the signal coming back."),
        ])
        .with_banks(&[("Gain", &["Out Gain", "In Gain", "Dry/Wet", "Invert"])]),
    )
}

/// Adds the External Audio Effect to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, hardware_effect_type(), |device, config| {
        Effect::new(device, config)
    });
}

/// The node: it holds the delay line its dry signal waits in and the peaks
/// its indicators show.
struct Effect {
    /// The dry signal's delay line: `latency` frames a channel.
    line: Vec<f32>,
    /// The frames the line holds, a channel's worth each.
    latency: usize,
    /// Where the line is written next.
    write: usize,
    /// The highest level the outgoing and the returning signal have reached.
    peaks: [f32; 2],
}

impl Effect {
    /// A node for `device`: its delay line holds the device's latency, the
    /// interface's round trip and its Hardware Latency (R19, R20).
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let hardware = device.external.as_ref().map_or(0, |external| {
            external.hardware_latency.frames(config.sample_rate.hz())
        });
        let latency = (config.round_trip_latency as usize).saturating_add(hardware as usize);
        Self {
            line: vec![0.0; latency * config.channels],
            latency,
            write: 0,
            peaks: [0.0; 2],
        }
    }
}

impl NativeDevice for Effect {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let gain_out = params.ramp(params.index(GAIN_OUT).expect("the description holds it"));
        let gain_in = params.ramp(params.index(GAIN_IN).expect("the description holds it"));
        let wet = params.ramp(params.index(DRY_WET).expect("the description holds it"));
        let invert = params.ramp(params.index(INVERT).expect("the description holds it"));
        let latency = self.latency;
        let frames = audio.frames();
        let channels = audio.channels();
        let mut write = self.write;
        let mut peak_out = self.peaks[0];
        let mut peak_in = self.peaks[1];
        for frame in 0..frames {
            for channel in 0..channels {
                // Spec: external-devices R18, R19. The signal leaves for the
                // hardware as it is, times the out Gain; the returning signal
                // comes back on the interface's inputs the engine mapped,
                // times the in Gain and with its phase flipped while Invert
                // is on.
                let returning = audio.external(channel);
                let (input, outgoing) = audio.channel_with_external_out(channel);
                let heard = input[frame];
                if let Some(outgoing) = outgoing {
                    let sent = heard * gain_out[frame];
                    peak_out = peak_out.max(sent.abs());
                    outgoing[frame] = sent;
                }
                let back = returning.map_or(0.0, |returning| {
                    let sample = returning[frame] * gain_in[frame];
                    if invert[frame] >= 0.5 {
                        -sample
                    } else {
                        sample
                    }
                });
                peak_in = peak_in.max(back.abs());
                // The dry signal waits in the line for the device's
                // latency, so it lines up with the returning signal.
                let dry = if latency == 0 {
                    heard
                } else {
                    let line = &mut self.line[channel * latency..(channel + 1) * latency];
                    let held = line[write];
                    line[write] = heard;
                    held
                };
                let mix = wet[frame] / 100.0;
                input[frame] = (1.0 - mix) * dry + mix * back;
            }
            // The line holds one frame for every channel, and the index
            // moves on once a frame, so the dry signal waits the device's
            // latency.
            if latency > 0 {
                write = (write + 1) % latency;
            }
        }
        self.write = write;
        self.peaks = [peak_out, peak_in];
    }

    fn display_len(&self) -> usize {
        2
    }

    fn display(&mut self, out: &mut [f32]) {
        // Spec: external-devices R21. The peak indicators show the highest
        // level the outgoing and the returning signal have reached.
        out[0] = self.peaks[0];
        out[1] = self.peaks[1];
    }
}
