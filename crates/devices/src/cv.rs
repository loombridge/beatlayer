//! What the CV tools share (WP-7.17; the CV tools spec, R1, R9, R10): control
//! voltages as sample values, the return CV Instrument and CV Triggers bring
//! back like the External Instrument, and their display readings.

use bl_model::{DeviceDisplay, DeviceId, DisplayReading, ParamDescriptor, ParamId};

/// The return's Gain, the first parameter of CV Instrument and CV Triggers.
pub const GAIN: ParamId = ParamId(0);

/// The return's Gain: −∞ to +24 dB, 0 dB on a new device, as the External
/// Instrument's (external devices R11; cv-tools R10).
pub(crate) fn gain() -> ParamDescriptor {
    ParamDescriptor::gain(GAIN, "Gain", 24.0, 0.0)
        .describe("Scales the sound that comes back from the hardware.")
}

/// The volts a sample value stands for: 1.0 is +10 V (R1).
pub(crate) const VOLTS_PER_SAMPLE: f64 = 10.0;

/// The sample value for `volts`, clamped to the interface's full scale (R1).
pub(crate) fn sample(volts: f64) -> f32 {
    (volts / VOLTS_PER_SAMPLE).clamp(-1.0, 1.0) as f32
}

/// The frames of the 1 ms gap a retrigger leaves (R13, R17).
pub(crate) fn gap_frames(rate: f64) -> u32 {
    (rate * 0.001).round().max(1.0) as u32
}

/// The frames of `milliseconds`.
pub(crate) fn frames_of(rate: f64, milliseconds: f64) -> u32 {
    (rate * milliseconds / 1000.0).round().max(0.0) as u32
}

/// A display of single readings in volts, one per output named in `names`,
/// from the first values of `data` (R9). Without `data` the device has
/// published nothing yet.
pub(crate) fn volts_display(
    device: DeviceId,
    names: impl IntoIterator<Item = String>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let readings = names
        .into_iter()
        .enumerate()
        .map(|(at, name)| {
            let value = data.and_then(|data| data.get(at)).copied().unwrap_or(0.0);
            DisplayReading {
                name,
                value,
                unit: "V".to_owned(),
                text: format!("{value:.2} V"),
            }
        })
        .collect();
    DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings,
    }
}
