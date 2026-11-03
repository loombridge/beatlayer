//! DS Cymbal (WP-8.25; the drum synths spec, R16): the hat's six
//! band-limited squares at a base of 500 Hz, with white noise, through a
//! high-pass, decaying over a long Decay. The manual gives the controls and
//! no ranges or curves, so the laws here are Beatlayer's own.

use bl_engine::{DeviceType, Registry};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, OrdMap, ParamDescriptor, ParamId,
    ParamUnit,
};

use crate::drum_synth::{DECAY, DrumSynth, Preset, VOLUME, decay, large, panel, tune, volume};
use crate::ds_hh::Hat;
use crate::framework::add;

/// DS Cymbal's device type id.
pub const DS_CYMBAL: &str = "beatlayer.ds_cymbal";

/// The high-pass's cutoff, in hertz.
pub const TONE: ParamId = crate::ds_hh::TONE;
/// The pitch of the squares, in per cent of two octaves each way (spec R5).
pub const PITCH: ParamId = crate::ds_hh::PITCH;

/// The base at Pitch 0.
const BASE_HZ: f64 = 500.0;

/// The DS Cymbal device type.
#[must_use]
pub fn ds_cymbal_type() -> DeviceType {
    let params = vec![
        decay(83.6),
        volume(),
        ParamDescriptor::continuous(TONE, "Tone", (1_000.0, 12_000.0), 4_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The high-pass's cutoff: higher values leave a thinner, brighter cymbal."),
        tune(PITCH, "Pitch", 50.0),
    ];
    DeviceType::native(DS_CYMBAL, "DS Cymbal", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(panel(
                (vec![large(TONE, 0)], Vec::new()),
                [PITCH, DECAY, VOLUME],
            ))
            .with_banks(&[
                ("Cymbal", &["Tone", "Pitch"]),
                ("Level", &["Decay", "Volume"]),
            ]),
    )
}

/// DS Cymbal's factory presets.
pub const PRESETS: [Preset; 3] = [
    Preset {
        name: "Crash",
        params: &[(TONE, 3_000.0), (DECAY, 90.0)],
    },
    Preset {
        name: "Ride",
        params: &[(TONE, 5_500.0), (PITCH, 58.3), (DECAY, 79.9)],
    },
    Preset {
        name: "Splash",
        params: &[(TONE, 7_000.0), (PITCH, 64.6), (DECAY, 65.3)],
    },
];

/// Adds DS Cymbal to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, ds_cymbal_type(), |device, config| {
        DrumSynth::new(device.id.0.get(), config.sample_rate.as_f64(), || {
            Hat::new(config.sample_rate.as_f64(), BASE_HZ, false)
        })
    });
}

/// DS Cymbal's display: the hit's envelope.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
) -> DeviceDisplay {
    crate::drum_synth::decay_display(device, description, values)
}
