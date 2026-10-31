//! A VST3 class's role in a chain from its subcategories, and its
//! controller's parameters as Beatlayer's descriptors.

use bl_model::{DeviceRole, ParamDescriptor, ParamId, ParamKind, ParamUnit, param::Taper};
use vst3::ComPtr;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;

use super::ParamMap;

/// One of the plug-in's parameters: its descriptor, and the plain range
/// and steps the host converts the device's plain values over.
pub(crate) struct Parameter {
    pub(crate) descriptor: ParamDescriptor,
    pub(crate) map: ParamMap,
}

/// Where a plug-in goes in a chain, from its class's subcategories: an
/// instrument, or else an audio effect. VST3 has no MIDI-effect category.
pub(crate) fn role(subcategories: &str) -> DeviceRole {
    if subcategories.split('|').any(|part| part == "Instrument") {
        DeviceRole::Instrument
    } else {
        DeviceRole::AudioEffect
    }
}

/// The controller's parameters that a person can set and automation can
/// move: every one that is automatable, not hidden, not read-only, not a
/// bypass, not a program change, and has a plain range. Each is a
/// continuous value over its range, or a whole number if it's stepped
/// (VST3 gives no unit).
pub(crate) fn parameters(controller: &ComPtr<IEditController>) -> Vec<Parameter> {
    use ParameterInfo_::ParameterFlags_ as Flags;
    let mut out: Vec<Parameter> = Vec::new();
    // SAFETY: the controller reports its own parameter count.
    let count = unsafe { controller.getParameterCount() }.max(0);
    for index in 0..count {
        let mut info: ParameterInfo = {
            // SAFETY: an all-zero ParameterInfo is a valid one; the
            // controller fills its fields.
            unsafe { std::mem::zeroed() }
        };
        // SAFETY: the controller reports the parameters by index.
        if unsafe { controller.getParameterInfo(index, &mut info) } != kResultOk {
            continue;
        }
        let flags = info.flags;
        if flags & Flags::kCanAutomate == 0
            || flags
                & (Flags::kIsHidden
                    | Flags::kIsReadOnly
                    | Flags::kIsBypass
                    | Flags::kIsProgramChange)
                != 0
        {
            continue;
        }
        let id = ParamId(info.id);
        if out.iter().any(|p| p.descriptor.id == id) {
            continue;
        }
        // SAFETY: the controller maps its own parameter's value, by ID.
        let (min, max) = unsafe {
            (
                controller.normalizedParamToPlain(info.id, 0.0),
                controller.normalizedParamToPlain(info.id, 1.0),
            )
        };
        if !(min.is_finite() && max.is_finite() && min < max) {
            continue;
        }
        // SAFETY: as above.
        let default = unsafe {
            controller.normalizedParamToPlain(info.id, info.defaultNormalizedValue.clamp(0.0, 1.0))
        };
        let name = utf16(&info.title);
        let name = name.trim().to_owned();
        let name = if name.is_empty() {
            format!("Parameter {}", index + 1)
        } else {
            name
        };
        // A stepped parameter whose plain range isn't its step count (the
        // SDK's own stepped parameters map normalized to plain as the
        // identity) is its step index, 0 to its step count.
        let indexed = info.stepCount > 0 && ((max - min) - f64::from(info.stepCount)).abs() > 1e-9;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "whole numbers in range, as in the CLAP host"
        )]
        let kind = if indexed {
            ParamKind::Integer {
                min: 0,
                max: i64::from(info.stepCount),
                unit: ParamUnit::None,
            }
        } else if info.stepCount > 0
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
        let (min, max, default) = if indexed {
            let steps = f64::from(info.stepCount);
            (
                0.0,
                steps,
                (info.defaultNormalizedValue.clamp(0.0, 1.0) * steps).round(),
            )
        } else {
            (min, max, default)
        };
        let default = if matches!(kind, ParamKind::Integer { .. }) {
            default.clamp(min, max).round()
        } else {
            default.clamp(min, max)
        };
        let descriptor = ParamDescriptor {
            id,
            name,
            description: String::new(),
            kind,
            default,
            automatable: true,
        };
        if descriptor.check().is_err() {
            continue;
        }
        let mut map = ParamMap::new(info.id, (min, max), default, info.stepCount, |normalized| {
            // SAFETY: as above.
            unsafe { controller.normalizedParamToPlain(info.id, normalized) }
        });
        map.step_indexed = indexed;
        out.push(Parameter { descriptor, map });
    }
    out
}

/// A `String128` (UTF-16) as a string, up to its terminator.
pub(crate) fn utf16(text: &String128) -> String {
    let len = text.iter().position(|c| *c == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..len])
}
