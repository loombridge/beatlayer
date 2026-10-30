//! CV Instrument's calibration at work (WP-7.17; the CV tools spec, R14):
//! the node steps through the voltages on the audio thread and publishes what
//! its Tuner's detector heard, and the command waits for the eight steps,
//! fits the line and stores the calibration in the device as one undo step.

use std::time::{Duration, Instant};

use bl_devices::CvCommand;
use bl_devices::cv_instrument::{
    CV_INSTRUMENT, FINISHED, FIRST_STEP_VOLTS, PITCHES_AT, RUN_AT, STATE_AT, STEPS,
};
use bl_model::{Command, CvCalibration, DeviceId, DeviceKind, External};
use bl_shell_spec::cv as ids;

use super::*;

/// How long the command waits for the calibration: eight steps of a quarter
/// of a second, with room for a busy machine.
const PATIENCE: Duration = Duration::from_secs(30);

impl Shell {
    /// Whether the CV tools' commands can run now.
    pub(super) fn can_cv(&self, id: &str, args: &Value) -> bool {
        let device = args
            .get("device")
            .and_then(|device| serde_json::from_value::<DeviceId>(device.clone()).ok())
            .or_else(|| self.devices.selected());
        let external = device
            .and_then(|device| self.editor.document().device(device))
            .and_then(|device| device.external.as_ref());
        match id {
            ids::CALIBRATE => external.is_some_and(|e| e.is_cv()) && self.audio.engine_running(),
            _ => external.is_some_and(|e| e.cv_calibration.is_some()),
        }
    }

    /// Runs one of the CV tools' commands.
    pub(super) fn run_cv(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ids::CvDeviceArgs { device } = Self::args(id, args)?;
        let (device, external) = self.cv_instrument(id, device)?;
        match id {
            ids::CALIBRATE => self.calibrate(id, device, &external),
            _ => {
                let cleared = external.cv_calibration.is_some();
                if cleared {
                    self.set_calibration(id, device, None)?;
                }
                Ok(to_value(&ids::CalibrationCleared { cleared }))
            }
        }
    }

    /// The CV Instrument a command acts on: the one given, or the selected
    /// device.
    fn cv_instrument(
        &self,
        id: &str,
        device: Option<DeviceId>,
    ) -> Result<(DeviceId, External), ShellError> {
        let device = device
            .or_else(|| self.devices.selected())
            .ok_or_else(|| self.failed(id, "select a CV Instrument first"))?;
        let found = self
            .editor
            .document()
            .device(device)
            .ok_or_else(|| self.failed(id, format!("no {device}")))?;
        match (&found.kind, &found.external) {
            (DeviceKind::Native { device_type }, Some(external))
                if device_type == CV_INSTRUMENT =>
            {
                Ok((device, external.clone()))
            }
            _ => Err(self.failed(id, format!("{device} isn't a CV Instrument"))),
        }
    }

    fn set_calibration(
        &mut self,
        id: &str,
        device: DeviceId,
        calibration: Option<CvCalibration>,
    ) -> Result<(), ShellError> {
        self.edit(|editor| {
            editor.apply(Command::SetCvCalibration {
                device,
                calibration,
            })
        })
        .map(|_| ())
        .map_err(|error| self.failed(id, error))
    }

    fn calibrate(
        &mut self,
        id: &str,
        device: DeviceId,
        external: &External,
    ) -> Result<Value, ShellError> {
        // Spec: cv-tools R14. Refused while Pitch is No Output or Audio From
        // is No Input.
        let pitch = external
            .cv_outputs
            .iter()
            .find(|port| port.name == "Pitch")
            .and_then(|port| port.channel);
        if pitch.is_none() {
            return Err(self.failed(id, "Pitch is set to No Output; choose a channel first"));
        }
        if external.audio_from == External::NO_INPUT {
            return Err(self.failed(
                id,
                "Audio From is set to No Input; choose the oscillator's input first",
            ));
        }
        if !self.audio.engine_running() {
            return Err(self.failed(
                id,
                "no audio engine is running; switch it on and open an audio device first",
            ));
        }
        // The engine plays the set as it is now before the node starts.
        if !self.audio.flush(Duration::from_secs(10)) {
            return Err(self.failed(id, "the audio engine did not take the set"));
        }
        let seen = |shell: &mut Shell| {
            shell.audio.displays_into(shell.devices.displays_mut());
            shell
                .devices
                .displays_mut()
                .get(&device)
                .map(|data| data.values.clone())
        };
        let before = seen(self).map_or(0.0, |values| values[RUN_AT]);
        self.audio
            .send_node_data(bl_engine::NodeKey::Device(device), CvCommand::Calibrate)
            .map_err(|error| self.failed(id, error))?;
        let started = Instant::now();
        let values = loop {
            match seen(self) {
                Some(values) if values[RUN_AT] > before && values[STATE_AT] == FINISHED => {
                    break values;
                }
                _ if started.elapsed() > PATIENCE => {
                    return Err(self.failed(id, "the calibration did not finish"));
                }
                _ => std::thread::sleep(Duration::from_millis(1)),
            }
        };
        let measured: Vec<f64> = values[PITCHES_AT..PITCHES_AT + STEPS]
            .iter()
            .map(|&hz| f64::from(hz))
            .collect();
        let calibration =
            CvCalibration::from_steps(FIRST_STEP_VOLTS, &measured).map_err(|step| {
                let volts = FIRST_STEP_VOLTS + step as f64;
                self.failed(
                    id,
                    format!(
                        "step {} ({volts:+} V) gave no steady pitch between 41 Hz and 2 kHz; the \
                     oscillator's sound must come back on Audio From",
                        step + 1
                    ),
                )
            })?;
        self.set_calibration(id, device, Some(calibration.clone()))?;
        Ok(to_value(&ids::Calibrated {
            device,
            calibration,
            measured,
        }))
    }
}
