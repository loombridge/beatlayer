//! Modular (WP-9.36; the modular device spec, R9): the command that reads a
//! device's patch back. The edits, `modular.add_block`,
//! `modular.remove_block`, `modular.connect`, `modular.disconnect`,
//! `modular.move_block`, `modular.set` and `modular.expose`, are
//! `bl-model`'s; each reads back through this one.

use bl_commands::Context::DeviceView;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::modular::{BlockId, PortType};
use bl_model::{DeviceId, ParamId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `modular.patch`: a Modular device's patch (R9).
pub const PATCH: &str = "modular.patch";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[PATCH];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `modular.patch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchArgs {
    /// The Modular device.
    pub device: DeviceId,
}

/// One port of a block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortView {
    /// The port's ID, which cables name.
    pub id: String,
    /// What it carries.
    #[serde(rename = "type")]
    pub kind: PortType,
}

/// One setting of a block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SettingView {
    /// The setting's ID, which `modular.set` names.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its value now.
    pub value: f64,
    /// Its value in a new block.
    pub default: f64,
    /// Its lowest value.
    pub min: f64,
    /// Its highest value.
    pub max: f64,
    /// The text after its number, with its space, or none.
    pub unit: String,
    /// The names of a choice's values, in order; empty for a number.
    pub choices: Vec<String>,
    /// The patch parameter it is, if it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u8>,
}

/// One block of the patch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BlockView {
    /// The block's ID in the patch.
    pub id: BlockId,
    /// Its type's ID.
    #[serde(rename = "type")]
    pub kind: String,
    /// Its type's name, or the type's ID where Beatlayer doesn't know it.
    pub name: String,
    /// Whether Beatlayer knows the type; a block it doesn't know is silent.
    pub known: bool,
    /// Where it sits in the editor.
    pub x: f32,
    /// Where it sits in the editor.
    pub y: f32,
    /// Its input ports.
    pub inputs: Vec<PortView>,
    /// Its output ports.
    pub outputs: Vec<PortView>,
    /// Its settings.
    pub settings: Vec<SettingView>,
}

/// One patch parameter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParamView {
    /// Its slot, from 1.
    pub slot: u8,
    /// The device parameter number `device.set_param` and automation take.
    pub param: ParamId,
    /// Its name: the block's, then the setting's.
    pub name: String,
    /// The block.
    pub block: BlockId,
    /// The setting's ID.
    pub setting: String,
}

/// What `modular.patch` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PatchState {
    /// The device.
    pub device: DeviceId,
    /// How many voices play, 1 to 32; 1 for an effect.
    pub voices: u32,
    /// The blocks, in the order they were added.
    pub blocks: Vec<BlockView>,
    /// The cables.
    pub cables: Vec<bl_model::modular::Cable>,
    /// The patch parameters, by slot. A value runs from 0 to 1 across its
    /// setting's range.
    pub params: Vec<ParamView>,
    /// How many blocks Beatlayer doesn't know, which play silence.
    pub unknown_blocks: usize,
}

/// Adds Modular's command to a registry.
///
/// # Errors
///
/// [`RegisterError`] if its ID is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(PATCH, "Modular Patch")
            .description(
                "Reports a Modular device's patch: every block with its type, place, ports (by ID \
                 and type) and settings (each with its value, default, range, unit and choices, \
                 and the patch parameter slot it is if it is one), the cables (each from a block's \
                 output port to another's input port), the patch parameters in slot order with \
                 the device parameter number each is, how many voices play, and how many blocks \
                 Beatlayer doesn't know. Changes nothing. The `modular.` commands that change the \
                 patch read back through it.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<PatchArgs>()
            .result::<PatchState>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_registers_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
    }
}
