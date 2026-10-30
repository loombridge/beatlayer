//! The CV tools' commands (WP-7.17; the CV tools spec, R14, R20): tuning the
//! oscillator patched to CV Instrument's Pitch against its sound coming back,
//! and taking the calibration away.
//!
//! `cv.calibrate` is in the `external` class, because it plays through the
//! audio interface.

use bl_commands::Context::DeviceView;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::{CvCalibration, DeviceId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `cv.calibrate`: calibrate a CV Instrument's Pitch (R14).
pub const CALIBRATE: &str = "cv.calibrate";
/// `cv.clear_calibration`: take a CV Instrument's calibration away (R14).
pub const CLEAR_CALIBRATION: &str = "cv.clear_calibration";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[CALIBRATE, CLEAR_CALIBRATION];

/// Whether `id` is one of these commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of both commands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CvDeviceArgs {
    /// The CV Instrument. The one selected in the Device View when it is left
    /// out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
}

/// What `cv.calibrate` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Calibrated {
    /// The device.
    pub device: DeviceId,
    /// The calibration stored: the line fitted and each step used, with its
    /// pitch and its error in cents.
    pub calibration: CvCalibration,
    /// The pitch each of the eight steps from −2 V to +5 V gave, in hertz, 0
    /// for a step the detector could not read or that was left out of the fit.
    pub measured: Vec<f64>,
}

/// What `cv.clear_calibration` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CalibrationCleared {
    /// Whether the device held a calibration.
    pub cleared: bool,
}

/// Registers the CV tools' commands.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(CALIBRATE, "Calibrate")
            .description(
                "Calibrates a CV Instrument against the oscillator patched to its Pitch output, \
                 whose sound must come back on its Audio From: with Gate high, it writes each \
                 whole volt from -2 V to +5 V on Pitch in turn, holds each for 250 ms and \
                 measures the returning pitch over the last 150 ms with the Tuner's detector, \
                 then fits the octaves the oscillator rises per volt and its frequency at 0 V \
                 by least squares on the logarithm of frequency, keeping each step's error so \
                 Pitch's law corrects it. The detector reads 41 Hz to 2 kHz, so the steps it \
                 cannot read, or that do not rise like an oscillator's, are left out of the fit \
                 and the keys beyond them follow the fitted line; three steps are needed. The \
                 calibration is stored in the device as one undo step and saves with the set; \
                 the result gives the fit, each step's error in cents and the pitch each step \
                 gave. It takes about two seconds and notes reaching the device meanwhile are \
                 ignored. It is refused while Pitch is No Output, Audio From is No Input or no \
                 audio engine runs, and fails, changing nothing, when too few steps give a \
                 steady pitch, saying which was the first. Read the result back in the query \
                 tool's `device` result, under `cv`.",
            )
            .permission(Permission::External)
            .context(DeviceView)
            .args::<CvDeviceArgs>()
            .result::<Calibrated>(),
    )?;
    registry.register(
        CommandSpec::new(CLEAR_CALIBRATION, "Clear Calibration")
            .description(
                "Takes a CV Instrument's calibration away, so Pitch writes one volt to the \
                 octave from key 60 at 0 V again. One undo step; it does nothing to a device \
                 that holds none.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<CvDeviceArgs>()
            .result::<CalibrationCleared>(),
    )?;
    Ok(())
}
