//! The Hardware Instrument (WP-5.34; the external devices spec, R9 to R13):
//! an instrument that makes no sound of its own. It sends the track's notes
//! to a MIDI output port, and brings the synthesiser's sound back from the
//! audio interface's inputs through a Gain.
//!
//! The engine does the routing: it sends the notes at the moment their
//! frame is heard at the output, maps the interface's inputs the device is
//! set to into its block, and reports the round trip plus its Hardware
//! Latency to delay compensation, all from the settings in the document
//! ([`bl_model::External`]). The node scales what comes back.

use bl_engine::{DeviceType, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// The Hardware Instrument's type id.
pub const HARDWARE_INSTRUMENT: &str = "beatlayer.hardware_instrument";

/// The level of the sound coming back.
pub const GAIN: ParamId = ParamId(0);

/// The Hardware Instrument's type: an instrument with routing to external
/// hardware.
#[must_use]
pub fn hardware_instrument_type() -> DeviceType {
    DeviceType::native(
        HARDWARE_INSTRUMENT,
        "Hardware Instrument",
        DeviceRole::Instrument,
    )
    .with_external()
    .with_description(DeviceDescription::new(vec![
        // Spec: external-devices R11. -inf to +24 dB, 0 dB on a new device.
        ParamDescriptor::gain(GAIN, "Gain", 24.0, 0.0)
            .describe("Scales the sound that comes back from the hardware."),
    ]))
}

/// Adds the Hardware Instrument to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, hardware_instrument_type(), |_, _| Hardware);
}

/// The node: it holds no state.
struct Hardware;

impl NativeDevice for Hardware {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let gain = params.ramp(params.index(GAIN).expect("the description holds it"));
        for channel in 0..audio.channels() {
            // Spec: external-devices R11. A pair comes back as left and
            // right and a single channel on both sides; the engine mapped
            // it, and left silence where the interface lacks the channel
            // (or no device is open).
            let returned = audio.external(channel);
            let out = audio.channel_mut(channel);
            match returned {
                Some(returned) => {
                    for ((sample, heard), gain) in out.iter_mut().zip(returned).zip(gain) {
                        *sample = heard * gain;
                    }
                }
                None => out.fill(0.0),
            }
        }
    }
}
