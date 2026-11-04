//! Modular's patch read back through the shell (WP-9.36; the modular device
//! spec, R9). The edits are `bl-model`'s commands; this is the one command
//! that reports what they made.

use bl_model::Device;
use bl_model::modular::{self, DEFAULT_VOICES, VOICES, slot_param};
use bl_shell_spec::modular::{
    self as ids, BlockView, ParamView, PatchArgs, PatchState, PortView, SettingView,
};
use serde_json::Value;

use super::*;

impl Shell {
    /// Runs `modular.patch`.
    pub(super) fn run_modular(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::PATCH => {
                let PatchArgs { device } = Self::args(id, args)?;
                let state = self
                    .editor
                    .document()
                    .device(device)
                    .and_then(patch_state)
                    .ok_or_else(|| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "the device isn't a Modular".to_owned(),
                    })?;
                Ok(to_value(&state))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}

/// `device`'s patch as `modular.patch` reports it, or `None` for a device
/// that isn't a Modular.
pub(super) fn patch_state(device: &Device) -> Option<PatchState> {
    let patch = &device.effective_patch()?;
    let ports = |ports: &[modular::PortSpec]| {
        ports
            .iter()
            .map(|p| PortView {
                id: p.id.to_owned(),
                kind: p.ty,
            })
            .collect()
    };
    let blocks = patch
        .blocks
        .iter()
        .map(|block| {
            let spec = block.spec();
            BlockView {
                id: block.id,
                kind: block.kind.clone(),
                name: spec.map_or_else(|| block.kind.clone(), |s| s.name.to_owned()),
                known: spec.is_some(),
                x: block.x,
                y: block.y,
                inputs: spec.map_or_else(Vec::new, |s| ports(s.inputs)),
                outputs: spec.map_or_else(Vec::new, |s| ports(s.outputs)),
                settings: spec.map_or_else(Vec::new, |s| {
                    s.settings
                        .iter()
                        .map(|setting| SettingView {
                            id: setting.id.to_owned(),
                            name: setting.name.to_owned(),
                            value: block.setting(setting.id).unwrap_or(setting.default),
                            default: setting.default,
                            min: setting.min,
                            max: setting.max,
                            unit: setting.unit.to_owned(),
                            choices: setting.choices.iter().map(|c| (*c).to_owned()).collect(),
                            slot: patch.param_of(block.id, setting.id).map(|p| p.slot),
                        })
                        .collect()
                }),
            }
        })
        .collect();
    let params = patch
        .params
        .iter()
        .filter_map(|param| {
            let block = patch.block(param.block)?;
            let setting = block.spec()?.setting(&param.setting)?;
            Some(ParamView {
                slot: param.slot,
                param: slot_param(param.slot),
                name: format!("{} {}", block.spec()?.name, setting.name),
                block: param.block,
                setting: param.setting.clone(),
            })
        })
        .collect();
    let voices = if matches!(&device.kind, bl_model::DeviceKind::Native { device_type } if device_type == modular::INSTRUMENT)
    {
        device
            .params
            .get(&VOICES)
            .map_or(DEFAULT_VOICES, |v| v.round() as u32)
    } else {
        1
    };
    Some(PatchState {
        device: device.id,
        voices,
        blocks,
        cables: patch.cables.clone(),
        params,
        unknown_blocks: patch.unknown_blocks(),
    })
}
