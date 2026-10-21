//! A CLAP plug-in's parameters as Beatlayer's descriptors, and its role in
//! a chain from its features.

use bl_model::{DeviceRole, ParamDescriptor, ParamId, ParamKind, ParamUnit, param::Taper};
use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::note_ports::PluginNotePorts;
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_host::prelude::*;

/// Where a plug-in goes in a chain, from its descriptor's features: an
/// instrument, a note effect that isn't also an audio effect (a MIDI
/// effect), or else an audio effect.
pub(crate) fn role<'a>(features: impl Iterator<Item = &'a std::ffi::CStr>) -> DeviceRole {
    let features: Vec<&[u8]> = features.map(std::ffi::CStr::to_bytes).collect();
    let has = |feature: &[u8]| features.contains(&feature);
    if has(b"instrument") {
        DeviceRole::Instrument
    } else if has(b"note-effect") && !has(b"audio-effect") {
        DeviceRole::MidiEffect
    } else {
        DeviceRole::AudioEffect
    }
}

/// Whether the plug-in sends notes on from a note output: it declares a
/// note output port (IMP-24). Such a plug-in can sit in the MIDI-effect
/// position of a MIDI track's chain, before the instrument.
pub(crate) fn note_output(plugin: &mut PluginMainThreadHandle<'_>) -> bool {
    plugin
        .get_extension::<PluginNotePorts>()
        .is_some_and(|ports| ports.count(plugin, false) > 0)
}

/// The plug-in's audio output ports past its main one (WP-5.45): the names
/// another track's Audio From chooser lists them under, empty when it has
/// only its main output.
///
/// # Panics
///
/// Never: a port with no name reads as `Output <n>`, counting from 1.
pub(crate) fn outputs(plugin: &mut PluginMainThreadHandle<'_>) -> Vec<String> {
    let Some(ext) = plugin.get_extension::<PluginAudioPorts>() else {
        return Vec::new();
    };
    let mut buffer = AudioPortInfoBuffer::new();
    let count = ext.count(plugin, false);
    let mut names = Vec::new();
    for index in 1..count {
        let info = ext.get(plugin, index, false, &mut buffer);
        let name = info
            .as_ref()
            .map(|info| String::from_utf8_lossy(info.name).trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("Output {}", index + 1));
        names.push(name);
    }
    names
}

/// The IDs of the parameters the plug-in marks modulatable: it takes
/// modulation as events of its own for them (the modulation spec, R10).
pub(crate) fn modulatable(plugin: &mut PluginMainThreadHandle<'_>) -> Vec<u32> {
    let Some(ext) = plugin.get_extension::<PluginParams>() else {
        return Vec::new();
    };
    let mut buffer = ParamInfoBuffer::new();
    let mut ids = Vec::new();
    for index in 0..ext.count(plugin) {
        if let Some(info) = ext.get_info(plugin, index, &mut buffer)
            && info.flags.contains(ParamInfoFlags::IS_MODULATABLE)
        {
            ids.push(info.id.get());
        }
    }
    ids
}

/// The plug-in's parameters that a person can set and automation can move:
/// every one that is automatable, not hidden, not read-only, and has a
/// range. Each is a continuous value over its range, or a whole number if
/// it's stepped (CLAP gives no unit).
pub(crate) fn params(plugin: &mut PluginMainThreadHandle<'_>) -> Vec<ParamDescriptor> {
    let Some(ext) = plugin.get_extension::<PluginParams>() else {
        return Vec::new();
    };
    let mut buffer = ParamInfoBuffer::new();
    let mut out: Vec<ParamDescriptor> = Vec::new();
    for index in 0..ext.count(plugin) {
        let Some(info) = ext.get_info(plugin, index, &mut buffer) else {
            continue;
        };
        let flags = info.flags;
        if !flags.contains(ParamInfoFlags::IS_AUTOMATABLE)
            || flags.intersects(ParamInfoFlags::IS_HIDDEN | ParamInfoFlags::IS_READONLY)
        {
            continue;
        }
        let (min, max) = (info.min_value, info.max_value);
        if !(min.is_finite() && max.is_finite() && min < max) {
            continue;
        }
        let id = ParamId(info.id.get());
        if out.iter().any(|p| p.id == id) {
            continue;
        }
        let name = String::from_utf8_lossy(info.name).trim().to_owned();
        let name = if name.is_empty() {
            format!("Parameter {}", index + 1)
        } else {
            name
        };
        let default = if info.default_value.is_finite() {
            info.default_value.clamp(min, max)
        } else {
            min
        };
        #[expect(clippy::cast_possible_truncation, reason = "whole numbers in range")]
        let kind = if flags.contains(ParamInfoFlags::IS_STEPPED)
            && min.fract() == 0.0
            && max.fract() == 0.0
            && min.abs() < 1e15
            && max.abs() < 1e15
        {
            ParamKind::Integer {
                min: min as i64,
                max: max as i64,
                unit: ParamUnit::None,
            }
        } else {
            ParamKind::Continuous {
                min,
                max,
                unit: ParamUnit::None,
                taper: Taper::Linear,
                decimals: None,
            }
        };
        let default = if matches!(kind, ParamKind::Integer { .. }) {
            default.round()
        } else {
            default
        };
        let descriptor = ParamDescriptor {
            id,
            name,
            description: String::new(),
            kind,
            default,
            automatable: true,
        };
        if descriptor.check().is_ok() {
            out.push(descriptor);
        }
    }
    out
}
