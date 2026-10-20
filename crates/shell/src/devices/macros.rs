//! A rack's macros on the shell (WP-5.4; the macros spec, R1 to R20):
//! reading a macro's name, value text and mappings, and the law that turns a
//! macro's position into each mapped parameter's value.
//!
//! The law needs each target parameter's descriptor, which lives here with
//! the device types, so the document's commands only store the mappings and
//! the shell writes the values they give.

use bl_model::modulation::Travel;
use bl_model::{
    Command, Device, DeviceId, Document, MacroMapping, MacroTarget, MixerParam, ParamId, ParamKind,
    ParamUnit, default_macro_name, macro_param,
};
use bl_shell_spec::devices::{MacroInfo, MacroMappingInfo, MacroState};

use super::DeviceView;
use crate::mixer_controls::{pan_text, volume_text};

/// A mapping's target travel, owned so it can outlive the document borrow.
pub(crate) enum OwnedTravel {
    /// A device parameter on its own taper.
    Param(bl_model::ParamDescriptor),
    /// A chain's Volume: the track fader's scale.
    Fader,
    /// A chain's Pan.
    Pan,
    /// A chain's Chain Activator.
    Switch,
}

impl OwnedTravel {
    fn travel(&self) -> Travel<'_> {
        match self {
            Self::Param(descriptor) => Travel::Param(descriptor),
            Self::Fader => Travel::Fader,
            Self::Pan => Travel::Pan,
            Self::Switch => Travel::Switch,
        }
    }

    /// The value at `position` on this travel.
    pub(crate) fn value(&self, position: f64) -> f64 {
        self.travel().value(position)
    }

    /// Where `value` sits on this travel.
    pub(crate) fn position(&self, value: f64) -> f64 {
        self.travel().position(value)
    }

    /// How `value` reads.
    fn text(&self, value: f64) -> String {
        match self {
            Self::Param(descriptor) => descriptor.format(value),
            Self::Fader => volume_text(value),
            Self::Pan => pan_text(value),
            Self::Switch => {
                if value >= 0.5 {
                    "On".to_owned()
                } else {
                    "Off".to_owned()
                }
            }
        }
    }
}

/// What a mapping points at: its travel and the names the mappings list
/// shows.
pub(crate) struct TargetInfo {
    pub travel: OwnedTravel,
    pub device: String,
    pub param: String,
}

impl DeviceView {
    /// What `target` maps, if the set still has it.
    pub(crate) fn target_info(&self, doc: &Document, target: MacroTarget) -> Option<TargetInfo> {
        match target {
            MacroTarget::Device { device, param } => {
                let device = doc.device(device)?;
                let description = self.description(&device.kind)?;
                let descriptor = description.param(param)?;
                Some(TargetInfo {
                    travel: OwnedTravel::Param(descriptor.clone()),
                    device: device.title().to_owned(),
                    param: descriptor.name.clone(),
                })
            }
            MacroTarget::Chain { chain, param } => {
                let chain = doc.chain(chain)?;
                let (param, travel) = match param {
                    MixerParam::Volume => ("Volume", OwnedTravel::Fader),
                    MixerParam::Pan => ("Pan", OwnedTravel::Pan),
                    MixerParam::Mute => ("Chain Activator", OwnedTravel::Switch),
                    // A chain has no split pan (the macros spec, R4).
                    MixerParam::SplitLeft | MixerParam::SplitRight => return None,
                };
                Some(TargetInfo {
                    travel,
                    device: chain.name.clone(),
                    param: param.to_owned(),
                })
            }
        }
    }

    /// A macro's value from 0 to 1.
    pub(crate) fn macro_value(rack: &Device, number: u8) -> f64 {
        rack.params
            .get(&macro_param(number))
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    }

    /// A macro's name (macros spec, R10): Rename's, else its one parameter's
    /// name, else "Macro N".
    pub(crate) fn macro_name(&self, doc: &Document, rack: &Device, number: u8) -> String {
        let settings = rack.macros.setting(number);
        if !settings.name.is_empty() {
            return settings.name.clone();
        }
        let mut mappings = rack.macros.mappings_of(number);
        if let Some(mapping) = mappings.next()
            && mappings.next().is_none()
            && let Some(info) = self.target_info(doc, mapping.target)
        {
            return info.param;
        }
        default_macro_name(number)
    }

    /// What a macro reads (macros spec, R10): its one parameter's value and
    /// unit, a shared unit, or 0 to 127.
    fn macro_text(&self, doc: &Document, value: f64, mappings: &[&MacroMapping]) -> String {
        if let [mapping] = mappings
            && let Some(info) = self.target_info(doc, mapping.target)
        {
            return info.travel.text(info.travel.value(mapping.position(value)));
        }
        if mappings.len() > 1
            && let Some(text) = self.shared_unit_text(doc, value, mappings)
        {
            return text;
        }
        format!("{}", (value * 127.0).round() as i64)
    }

    /// The text two or more mappings read in when their parameters share a
    /// kind, a unit and a range (macros spec, R10).
    fn shared_unit_text(
        &self,
        doc: &Document,
        value: f64,
        mappings: &[&MacroMapping],
    ) -> Option<String> {
        let descriptors: Vec<bl_model::ParamDescriptor> = mappings
            .iter()
            .filter_map(
                |mapping| match self.target_info(doc, mapping.target)?.travel {
                    OwnedTravel::Param(descriptor) => Some(descriptor),
                    _ => None,
                },
            )
            .collect();
        if descriptors.len() != mappings.len() {
            return None;
        }
        let first = descriptors.first()?;
        if descriptors
            .iter()
            .all(|descriptor| same_scale(first, descriptor))
        {
            Some(first.format(first.denormalize(value)))
        } else {
            None
        }
    }

    /// One macro as `rack.macros` reports it.
    pub(crate) fn macro_info(&self, doc: &Document, rack: &Device, number: u8) -> MacroInfo {
        let settings = rack.macros.setting(number);
        let value = Self::macro_value(rack, number);
        let mappings: Vec<&MacroMapping> = rack.macros.mappings_of(number).collect();
        let rows: Vec<MacroMappingInfo> = mappings
            .iter()
            .filter_map(|mapping| {
                let info = self.target_info(doc, mapping.target)?;
                Some(MacroMappingInfo {
                    target: mapping.target,
                    device: info.device,
                    param: info.param,
                    min: info.travel.value(mapping.min),
                    max: info.travel.value(mapping.max),
                    min_position: mapping.min,
                    max_position: mapping.max,
                })
            })
            .collect();
        MacroInfo {
            number,
            name: self.macro_name(doc, rack, number),
            colour: settings.colour,
            value,
            text: self.macro_text(doc, value, &mappings),
            randomize_excluded: settings.randomize_excluded,
            variation_excluded: settings.variation_excluded,
            mappings: rows,
        }
    }

    /// Every macro of `rack` as `rack.macros` reports them.
    pub(crate) fn macro_state(
        &self,
        doc: &Document,
        rack: DeviceId,
        count: u8,
    ) -> Option<MacroState> {
        let device = doc.device(rack)?;
        device.rack_kind()?;
        Some(MacroState {
            rack,
            count,
            macros: (1..=bl_model::MACROS)
                .map(|number| self.macro_info(doc, device, number))
                .collect(),
            variations: device
                .macros
                .variations
                .iter()
                .map(|variation| bl_shell_spec::devices::VariationInfo {
                    name: variation.name.clone(),
                    values: variation.values.clone(),
                })
                .collect(),
        })
    }

    /// The command that gives `mapping`'s target the value the macro gives
    /// it now (macros spec, R7).
    pub(crate) fn mapping_command(
        &self,
        doc: &Document,
        rack: &Device,
        mapping: &MacroMapping,
    ) -> Option<Command> {
        let info = self.target_info(doc, mapping.target)?;
        let value = info
            .travel
            .value(mapping.position(Self::macro_value(rack, mapping.macro_number)));
        Some(Command::SetMappedValue {
            target: mapping.target,
            value,
        })
    }

    /// The value `target` has now, in its own units.
    pub(crate) fn target_value(&self, doc: &Document, target: MacroTarget) -> Option<f64> {
        match target {
            MacroTarget::Device { device, param } => {
                let device = doc.device(device)?;
                let description = self.description(&device.kind)?;
                description.value(&device.params, param)
            }
            MacroTarget::Chain { chain, param } => {
                let chain = doc.chain(chain)?;
                Some(match param {
                    MixerParam::Volume => chain.mixer.volume,
                    MixerParam::Pan => chain.mixer.pan,
                    MixerParam::Mute => f64::from(u8::from(!chain.mixer.enabled)),
                    // A chain has no split pan (the macros spec, R4).
                    MixerParam::SplitLeft | MixerParam::SplitRight => return None,
                })
            }
        }
    }

    /// The commands that give every parameter macro `number` drives the
    /// value it gives it now.
    pub(crate) fn macro_commands(&self, doc: &Document, rack: &Device, number: u8) -> Vec<Command> {
        rack.macros
            .mappings_of(number)
            .filter_map(|mapping| self.mapping_command(doc, rack, mapping))
            .collect()
    }

    /// The commands that give the parameter of `target` the value its macro
    /// gives it now.
    pub(crate) fn target_commands(
        &self,
        doc: &Document,
        rack: &Device,
        target: MacroTarget,
    ) -> Vec<Command> {
        rack.macros
            .mapping(target)
            .and_then(|mapping| self.mapping_command(doc, rack, mapping))
            .into_iter()
            .collect()
    }

    /// The commands Randomize just moved the targets of (macros spec, R15):
    /// every macro with a mapping that isn't excluded.
    pub(crate) fn randomized_commands(&self, doc: &Document, rack: &Device) -> Vec<Command> {
        (1..=bl_model::MACROS)
            .filter(|number| rack.macros.has_mappings(*number))
            .filter(|number| !rack.macros.setting(*number).randomize_excluded)
            .flat_map(|number| self.macro_commands(doc, rack, number))
            .collect()
    }

    /// The macro that holds `device`'s `param`, by name, if one does (macros
    /// spec, R11).
    pub(crate) fn mapped_hold(
        &self,
        doc: &Document,
        device: DeviceId,
        param: ParamId,
    ) -> Option<String> {
        let target = MacroTarget::Device { device, param };
        let rack = doc
            .tracks()
            .flat_map(|track| track.devices_deep())
            .filter(|rack| rack.rack_kind().is_some())
            .find(|rack| rack.macros.mapping(target).is_some())?;
        let mapping = rack.macros.mapping(target)?;
        Some(self.macro_name(doc, rack, mapping.macro_number))
    }
}

/// Whether two parameters share a kind, a unit and a range (macros spec,
/// R10).
fn same_scale(a: &bl_model::ParamDescriptor, b: &bl_model::ParamDescriptor) -> bool {
    a.min() == b.min()
        && a.max() == b.max()
        && a.is_stepped() == b.is_stepped()
        && unit_of(a) == unit_of(b)
        && std::mem::discriminant(&a.kind) == std::mem::discriminant(&b.kind)
}

/// A parameter's unit, where its kind has one.
fn unit_of(descriptor: &bl_model::ParamDescriptor) -> Option<ParamUnit> {
    match descriptor.kind {
        ParamKind::Continuous { unit, .. } | ParamKind::Integer { unit, .. } => Some(unit),
        _ => None,
    }
}
