//! CV Utility (WP-7.18; the CV tools spec, R28, R29, R30): an audio effect
//! that treats signals as control voltages. Its two inputs are A, the track's
//! signal at its place in the chain, and B, the source its sidechain section
//! chooses, each averaged over both sides and scaled by a Gain. Mode adds
//! them or multiplies them as shares of full scale, Offset is added after,
//! and the result, within ±10 V, replaces the track's signal on every side
//! and goes to the device's CV output.

use bl_engine::{DeviceType, Registry};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::cv::{VOLTS_PER_SAMPLE, volts_display};
use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// CV Utility's type id.
pub const CV_UTILITY: &str = "beatlayer.cv_utility";

/// A's Gain.
pub const GAIN_A: ParamId = ParamId(0);
/// B's Gain.
pub const GAIN_B: ParamId = ParamId(1);
/// Mode: Add or Multiply.
pub const MODE: ParamId = ParamId(2);
/// Offset: added to the result, from −10 to +10 V.
pub const OFFSET: ParamId = ParamId(3);

/// The modes, in the order of the chooser (R28).
pub const MODES: [&str; 2] = ["Add", "Multiply"];

/// The output's name in the device's routing.
pub const OUTPUT_NAMES: [&str; 1] = ["CV"];

/// CV Utility's type.
#[must_use]
pub fn cv_utility_type() -> DeviceType {
    let gain = |id, name: &str, what: &str| {
        ParamDescriptor::continuous(id, name, (-200.0, 200.0), 100.0, ParamUnit::Percent)
            .decimals(0)
            .describe(what)
    };
    let params = vec![
        gain(
            GAIN_A,
            "Gain A",
            "Scales the track's signal; negative inverts it.",
        ),
        gain(
            GAIN_B,
            "Gain B",
            "Scales the sidechain's signal; negative inverts it.",
        ),
        ParamDescriptor::choice(MODE, "Mode", &MODES, 0)
            .describe("Add gives A + B. Multiply gives A times B as a share of full scale."),
        ParamDescriptor::continuous(OFFSET, "Offset", (-10.0, 10.0), 0.0, ParamUnit::None)
            .decimals(2)
            .describe("The voltage added to the result, in volts."),
    ];
    DeviceType::native(CV_UTILITY, "CV Utility", DeviceRole::AudioEffect)
        .with_sidechain()
        .with_cv(&OUTPUT_NAMES, &[])
        .with_description(DeviceDescription::new(params))
}

/// Adds CV Utility to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, cv_utility_type(), |_, _| CvUtility::default());
}

/// CV Utility's display (R9, R30): the voltage the output last wrote.
#[must_use]
pub fn display(device: DeviceId, data: Option<&[f32]>) -> DeviceDisplay {
    volts_display(device, OUTPUT_NAMES.map(str::to_owned), data)
}

#[derive(Default)]
struct CvUtility {
    shown: f32,
}

impl NativeDevice for CvUtility {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let ramp = |id| params.ramp(params.index(id).expect("the description holds it"));
        let (gain_a, gain_b, mode, offset) = (ramp(GAIN_A), ramp(GAIN_B), ramp(MODE), ramp(OFFSET));
        let channels = audio.channels();
        let share = 1.0 / channels as f32;
        for f in 0..audio.frames() {
            let a = (0..channels).map(|c| audio.channel_mut(c)[f]).sum::<f32>() * share * gain_a[f]
                / 100.0;
            // Spec: cv-tools R28. B is nothing while the sidechain is off.
            let b = (0..channels)
                .filter_map(|c| audio.sidechain(c).map(|side| side[f]))
                .sum::<f32>()
                * share
                * gain_b[f]
                / 100.0;
            let combined = if mode[f] >= 0.5 { a * b } else { a + b };
            let result = (combined + offset[f] / VOLTS_PER_SAMPLE as f32).clamp(-1.0, 1.0);
            for c in 0..channels {
                audio.channel_mut(c)[f] = result;
            }
            if let Some(out) = audio.cv_out(0) {
                out[f] = result;
            }
            self.shown = result * VOLTS_PER_SAMPLE as f32;
        }
    }

    fn display_len(&self) -> usize {
        OUTPUT_NAMES.len()
    }

    fn display(&mut self, out: &mut [f32]) {
        out[0] = self.shown;
    }
}
