//! Operator's commands (WP-8.13; the Operator spec, R27, R28): drawing a
//! user wave's partials, copying an oscillator, setting an envelope's
//! levels, Play By Key and the harmonics editor's options.

use bl_devices::operator::{
    ENV_INITIAL, ENV_PEAK, ENV_SUSTAIN, FILTER_ENV, FILTER_FREQ, FREQ_KEY, LFO_ENV, OPERATOR,
    OSC_REPEAT, OSC_REPEAT_ON, PITCH_ENV, SHAPED_END, SHAPED_INITIAL, SHAPED_PEAK, SHAPED_SUSTAIN,
    osc, shaped,
};
use bl_model::{Command, Device, DeviceId, DeviceKind, Document, Editor, ParamId};
use bl_shell_spec::devices::{
    CopyOscillatorArgs, EnvelopeLevel, HarmonicsView, OperatorChanged, OscillatorCopied,
    PartialsChanged, PlayByKeyArgs, SetEnvelopeLevelsArgs, SetHarmonicsArgs, SetPartialsArgs,
};

use super::{DeviceError, DeviceView, parse};

/// Play By Key's cutoff, in hertz (operator R28).
const PLAY_BY_KEY_HZ: f64 = 466.0;

/// An oscillator's controls, from On to its envelope's Time < Vel, and
/// Repeat's switch: what Copy Oscillator copies.
fn osc_controls() -> impl Iterator<Item = u32> {
    (0..=bl_devices::operator::ENV_TIME_VEL).chain([OSC_REPEAT_ON])
}

/// A new user wave's partials: the first alone.
fn first_partial() -> Vec<f32> {
    let mut levels = vec![0.0; bl_model::partials::COUNTS[0]];
    levels[0] = 1.0;
    levels
}

fn oscillator(op: u8) -> Result<usize, DeviceError> {
    if op < bl_model::partials::OSCILLATORS {
        Ok(usize::from(op))
    } else {
        Err(DeviceError::InvalidArgs(format!(
            "oscillators are 0 for A to 3 for D, not {op}"
        )))
    }
}

impl DeviceView {
    /// The Operator a command names, or the selected one.
    fn operator(&self, doc: &Document, device: Option<DeviceId>) -> Result<DeviceId, DeviceError> {
        let device = self.target(doc, device)?;
        let current = doc.device(device).expect("the target exists");
        if matches!(&current.kind, DeviceKind::Native { device_type } if device_type == OPERATOR) {
            Ok(device)
        } else {
            Err(DeviceError::Unavailable(format!(
                "{device} isn't an Operator"
            )))
        }
    }

    /// `device.set_partials`: the wave's partials and count, and Repeat,
    /// in one undo step (operator R27).
    pub(super) fn set_partials(
        &self,
        args: serde_json::Value,
        editor: &mut Editor,
    ) -> Result<PartialsChanged, DeviceError> {
        let args: SetPartialsArgs = parse(args)?;
        let device = self.operator(editor.document(), args.device)?;
        let op = oscillator(args.osc)?;
        let count = args.levels.len();
        let repeat_on = args.repeat_on.or(args.repeat.map(|_| true));
        editor
            .transact("Change Partials", |editor| {
                editor.apply(Command::SetPartials {
                    device,
                    osc: args.osc,
                    levels: args.levels,
                })?;
                if let Some(repeat) = args.repeat {
                    editor.apply(Command::SetDeviceParam {
                        device,
                        param: osc(op, OSC_REPEAT),
                        value: repeat.clamp(0.0, 100.0),
                    })?;
                }
                if let Some(on) = repeat_on {
                    editor.apply(Command::SetDeviceParam {
                        device,
                        param: osc(op, OSC_REPEAT_ON),
                        value: f64::from(u8::from(on)),
                    })?;
                }
                Ok::<(), bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::InvalidArgs(e.to_string()))?;
        Ok(PartialsChanged {
            device,
            osc: args.osc,
            count,
        })
    }

    /// `operator.copy_oscillator`: every control of one oscillator and its
    /// partials to another, in one undo step (operator R28).
    pub(super) fn copy_oscillator(
        &self,
        args: serde_json::Value,
        editor: &mut Editor,
    ) -> Result<OscillatorCopied, DeviceError> {
        let args: CopyOscillatorArgs = parse(args)?;
        let device = self.operator(editor.document(), args.device)?;
        let (from, to) = (oscillator(args.from)?, oscillator(args.to)?);
        let current = editor.document().device(device).expect("the target exists");
        let description = &self
            .device_type(&current.kind)
            .expect("Operator is registered")
            .description;
        let moves: Vec<(ParamId, f64)> = osc_controls()
            .map(|control| {
                let value = description
                    .value(&current.params, osc(from, control))
                    .unwrap_or(0.0);
                (osc(to, control), value)
            })
            .collect();
        let levels = current
            .partials
            .iter()
            .find(|p| usize::from(p.osc) == from)
            .map_or_else(first_partial, |p| p.levels.clone());
        editor
            .transact("Copy Oscillator", |editor| {
                for (param, value) in moves {
                    editor.apply(Command::SetDeviceParam {
                        device,
                        param,
                        value,
                    })?;
                }
                editor.apply(Command::SetPartials {
                    device,
                    osc: args.to,
                    levels,
                })?;
                Ok::<(), bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        Ok(OscillatorCopied {
            device,
            from: args.from,
            to: args.to,
        })
    }

    /// `operator.set_envelope_levels`: an envelope's levels to their
    /// maximum, minimum or middle, in one undo step (operator R28).
    pub(super) fn set_envelope_levels(
        &self,
        args: serde_json::Value,
        editor: &mut Editor,
    ) -> Result<OperatorChanged, DeviceError> {
        let args: SetEnvelopeLevelsArgs = parse(args)?;
        let device = self.operator(editor.document(), args.device)?;
        let name = args.envelope.to_ascii_lowercase();
        // An oscillator's levels are amplitudes; the others' run from -100
        // % to 100 %.
        let amplitude = |top, bottom, middle| match args.to {
            EnvelopeLevel::Maximum => top,
            EnvelopeLevel::Minimum => bottom,
            EnvelopeLevel::Middle => middle,
        };
        let moves: Vec<(ParamId, f64)> = match name.as_str() {
            "a" | "b" | "c" | "d" => {
                let op = usize::from(name.as_bytes()[0] - b'a');
                let level = amplitude(1.0, 0.0, 0.5);
                [ENV_INITIAL, ENV_PEAK, ENV_SUSTAIN]
                    .into_iter()
                    .map(|c| (osc(op, c), level))
                    .collect()
            }
            "filter" | "pitch" | "lfo" => {
                let base = match name.as_str() {
                    "filter" => FILTER_ENV,
                    "pitch" => PITCH_ENV,
                    _ => LFO_ENV,
                };
                let level = amplitude(100.0, -100.0, 0.0);
                [SHAPED_INITIAL, SHAPED_PEAK, SHAPED_SUSTAIN, SHAPED_END]
                    .into_iter()
                    .map(|c| (shaped(base, c), level))
                    .collect()
            }
            _ => {
                return Err(DeviceError::InvalidArgs(format!(
                    "`envelope` is A, B, C, D, Filter, Pitch or LFO, not {:?}",
                    args.envelope
                )));
            }
        };
        editor
            .transact("Set All Envelope Levels", |editor| {
                for (param, value) in moves {
                    editor.apply(Command::SetDeviceParam {
                        device,
                        param,
                        value,
                    })?;
                }
                Ok::<(), bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        Ok(OperatorChanged { device })
    }

    /// `operator.play_by_key`: the cutoff to 466 Hz and Freq < Key to 100
    /// %, in one undo step (operator R28).
    pub(super) fn play_by_key(
        &self,
        args: serde_json::Value,
        editor: &mut Editor,
    ) -> Result<OperatorChanged, DeviceError> {
        let args: PlayByKeyArgs = parse(args)?;
        let device = self.operator(editor.document(), args.device)?;
        editor
            .transact("Play By Key", |editor| {
                for (param, value) in [(FILTER_FREQ, PLAY_BY_KEY_HZ), (FREQ_KEY, 100.0)] {
                    editor.apply(Command::SetDeviceParam {
                        device,
                        param,
                        value,
                    })?;
                }
                Ok::<(), bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        Ok(OperatorChanged { device })
    }

    /// `operator.set_harmonics`: the harmonics editor's options, which
    /// are view state (operator R27).
    pub(super) fn set_harmonics(
        &mut self,
        args: serde_json::Value,
        doc: &Document,
    ) -> Result<HarmonicsView, DeviceError> {
        let args: SetHarmonicsArgs = parse(args)?;
        let device = self.operator(doc, args.device)?;
        let mut view = self.harmonics(device);
        if let Some(on) = args.even_only {
            view.even_only = on;
            view.odd_only &= !on;
        }
        if let Some(on) = args.odd_only {
            view.odd_only = on;
            view.even_only &= !on;
        }
        if let Some(on) = args.normalize {
            view.normalize = on;
        }
        self.harmonics.insert(device, view);
        Ok(view)
    }

    /// The harmonics editor's options for `device`: Normalize on, and the
    /// even and odd partials both drawn, as a new device has them.
    #[must_use]
    pub fn harmonics(&self, device: DeviceId) -> HarmonicsView {
        self.harmonics
            .get(&device)
            .copied()
            .unwrap_or(HarmonicsView {
                device,
                even_only: false,
                odd_only: false,
                normalize: true,
            })
    }

    /// Operator's display for `view` (operator R29).
    pub(super) fn operator_display(device: &Device, view: Option<&str>) -> bl_model::DeviceDisplay {
        bl_devices::operator_display::display(device.id, &device.params, &device.partials, view)
    }
}
