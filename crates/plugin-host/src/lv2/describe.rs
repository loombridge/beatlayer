//! An LV2 plug-in as a device type (the LV2 hosting spec, R1 and R5): its
//! role in a chain from its class and ports, and its control input ports as
//! the device's parameters.

use bl_engine::DeviceType;
use bl_model::{
    DeviceDescription, DeviceKind, DeviceRole, ParamDescriptor, ParamId, ParamKind, ParamUnit,
    PluginFormat, param::Taper,
};

use super::{PluginInfo, PortInfo, PortKind};

/// One of the plug-in's parameters: its descriptor, and how the device's
/// value maps to the port's.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Parameter {
    pub(crate) descriptor: ParamDescriptor,
    pub(crate) map: ParamMap,
}

/// A parameter's place in the plug-in: the control port it is, the port's
/// range, and the offset between the port's value and the device's (a
/// choice is counted from 0, whatever values the plug-in's scale points
/// have).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ParamMap {
    pub(crate) port: u32,
    pub(crate) offset: f64,
    pub(crate) min: f64,
    pub(crate) max: f64,
    /// The parameter's default, in the device's units.
    pub(crate) default: f64,
}

impl ParamMap {
    /// The port's value for the device's `plain` one.
    pub(crate) fn port_value(&self, plain: f64) -> f32 {
        (plain + self.offset).clamp(self.min, self.max) as f32
    }

    /// The device's value for the port's.
    pub(crate) fn plain_value(&self, value: f32) -> f64 {
        f64::from(value) - self.offset
    }
}

/// What `units:unit` names, by the part of its URI after `units#`; none for
/// a unit the device can't show.
pub(super) fn unit(name: &str) -> ParamUnit {
    match name {
        "db" => ParamUnit::Decibels,
        "hz" => ParamUnit::Hertz,
        "ms" => ParamUnit::Milliseconds,
        "s" => ParamUnit::Seconds,
        "pc" => ParamUnit::Percent,
        "semitone12TET" => ParamUnit::Semitones,
        "cent" => ParamUnit::Cents,
        _ => ParamUnit::None,
    }
}

/// Where a plug-in goes in a chain: an instrument by its class, a MIDI
/// plug-in with no audio output a MIDI effect, else an audio effect.
pub(crate) fn role(info: &PluginInfo) -> DeviceRole {
    let audio_out = info
        .ports
        .iter()
        .any(|p| p.kind == PortKind::Audio && !p.input);
    let midi = info
        .ports
        .iter()
        .any(|p| matches!(p.kind, PortKind::Atom { midi: true }));
    if info.instrument {
        DeviceRole::Instrument
    } else if midi && !audio_out {
        DeviceRole::MidiEffect
    } else {
        DeviceRole::AudioEffect
    }
}

/// Whether the plug-in sends notes on from a MIDI output port: it can sit
/// in the MIDI-effect position of a MIDI track's chain.
pub(crate) fn note_output(info: &PluginInfo) -> bool {
    info.ports
        .iter()
        .any(|p| matches!(p.kind, PortKind::Atom { midi: true }) && !p.input)
}

/// The values a choice's scale points have, if they count up by one from
/// the first: its first value then.
fn consecutive(points: &[(f32, String)]) -> Option<f64> {
    let first = f64::from(points.first()?.0);
    if first.fract() != 0.0 {
        return None;
    }
    points
        .iter()
        .enumerate()
        .all(|(at, (value, _))| f64::from(*value) == first + at as f64)
        .then_some(first)
}

fn parameter(port: &PortInfo) -> Option<Parameter> {
    if port.kind != PortKind::Control
        || !port.input
        || port.hidden
        || port.not_automatic
        || !(port.min.is_finite() && port.max.is_finite() && port.default.is_finite())
    {
        return None;
    }
    let name = port.name.trim();
    let name = if name.is_empty() {
        port.symbol.clone()
    } else {
        name.to_owned()
    };
    let (min, max) = (f64::from(port.min), f64::from(port.max));
    let (kind, offset, range, default) = if port.toggled {
        (
            ParamKind::Toggle,
            0.0,
            (0.0, 1.0),
            f64::from(port.default > 0.0),
        )
    } else if port.enumeration
        && port.scale_points.len() > 1
        && let Some(first) = consecutive(&port.scale_points)
    {
        let last = port.scale_points.len() - 1;
        let default = (f64::from(port.default) - first)
            .clamp(0.0, last as f64)
            .round();
        (
            ParamKind::Choice {
                choices: port.scale_points.iter().map(|(_, l)| l.clone()).collect(),
            },
            first,
            (first, first + last as f64),
            default,
        )
    } else if min >= max {
        return None;
    } else if port.integer && min.abs() < 1e15 && max.abs() < 1e15 {
        (
            ParamKind::Integer {
                min: min.ceil() as i64,
                max: max.floor() as i64,
                unit: port.unit,
            },
            0.0,
            (min, max),
            f64::from(port.default).clamp(min, max).round(),
        )
    } else {
        let taper = if port.logarithmic && min > 0.0 {
            Taper::Logarithmic
        } else {
            Taper::Linear
        };
        (
            ParamKind::Continuous {
                min,
                max,
                unit: port.unit,
                taper,
                decimals: None,
            },
            0.0,
            (min, max),
            f64::from(port.default).clamp(min, max),
        )
    };
    let descriptor = ParamDescriptor {
        id: ParamId(port.index),
        name,
        description: String::new(),
        kind,
        default,
        automatable: true,
    };
    descriptor.check().ok()?;
    Some(Parameter {
        descriptor,
        map: ParamMap {
            port: port.index,
            offset,
            min: range.0,
            max: range.1,
            default,
        },
    })
}

/// The plug-in's parameters: every control input that isn't kept off the
/// plug-in's panel or from automation, and has a range (a toggle or a
/// choice needs none).
pub(crate) fn parameters(info: &PluginInfo) -> Vec<Parameter> {
    info.ports.iter().filter_map(parameter).collect()
}

/// The device type of `info` (R1), with its parameters.
pub(crate) fn device_type(info: &PluginInfo) -> (DeviceType, Vec<Parameter>) {
    let params = parameters(info);
    let device_type = DeviceType {
        external_output: false,
        cv_outputs: Vec::new(),
        cv_inputs: Vec::new(),
        kind: DeviceKind::Plugin {
            format: PluginFormat::Lv2,
            plugin_id: info.uri.clone(),
        },
        name: info.name.clone(),
        role: role(info),
        description: DeviceDescription::new(params.iter().map(|p| p.descriptor.clone()).collect()),
        sidechain: false,
        external: false,
        midi_sidechain: false,
        note_output: note_output(info),
        outputs: Vec::new(),
    };
    (device_type, params)
}
