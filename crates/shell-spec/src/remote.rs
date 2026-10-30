//! MIDI and key mappings, and the remote input (WP-7.1; the remote mapping
//! spec, R14): `remote.mappings`, `remote.map`, `remote.unmap`,
//! `remote.set_range`, `remote.invert_range`, `remote.set_mode` and
//! `remote.input`, with their arguments and results.
//!
//! A mapping is named by its source, which is unique in a set (R5), so the
//! commands that change one take the source `remote.mappings` reports.
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them
//! by ID.

use bl_commands::Context::Global;
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{ControlKind, RemoteMode, RemoteSource, RemoteTarget};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `remote.mappings`: list the set's MIDI and key mappings.
pub const MAPPINGS: &str = "remote.mappings";
/// `remote.map`: make a mapping.
pub const MAP: &str = "remote.map";
/// `remote.unmap`: delete mappings.
pub const UNMAP: &str = "remote.unmap";
/// `remote.set_range`: set a mapping's Min and Max.
pub const SET_RANGE: &str = "remote.set_range";
/// `remote.invert_range`: swap a mapping's Min and Max.
pub const INVERT_RANGE: &str = "remote.invert_range";
/// `remote.set_mode`: set a controller mapping's mode.
pub const SET_MODE: &str = "remote.set_mode";
/// `remote.input`: apply a MIDI message or a key as if it came from the
/// remote input.
pub const INPUT: &str = "remote.input";

/// `remote.map_mode`: turn MIDI Map Mode or Key Map Mode on or off.
pub const MAP_MODE: &str = "remote.map_mode";
/// `remote.select`: select the control the next message or key maps.
pub const SELECT: &str = "remote.select";
/// `remote.state`: the map mode, the selection, the guess and the Mapping
/// Browser's rows.
pub const STATE: &str = "remote.state";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    MAPPINGS,
    MAP,
    UNMAP,
    SET_RANGE,
    INVERT_RANGE,
    SET_MODE,
    INPUT,
    MAP_MODE,
    SELECT,
    STATE,
];

/// Whether `id` is one of the remote mapping commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// One mapping as `remote.mappings` reports it (R14).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MappingInfo {
    /// Its source; with a note range, the range's root.
    pub source: RemoteSource,
    /// The control, named by IDs.
    pub target: RemoteTarget,
    /// The path to the control: its track and device, or its view.
    pub path: String,
    /// The control's name.
    pub name: String,
    /// The control's kind.
    pub kind: ControlKind,
    /// The bottom of the range, in the control's own units.
    pub min: f64,
    /// The top of the range.
    pub max: f64,
    /// The lowest and highest values the control's own units allow Min and
    /// Max (map modes R9).
    pub range: (f64, f64),
    /// How a controller's values are read.
    pub mode: RemoteMode,
}

/// What `remote.mappings` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MappingsResult {
    /// Every mapping, MIDI then keys, in the order the set keeps them.
    pub mappings: Vec<MappingInfo>,
    /// How many mappings the set's load dropped because their control isn't
    /// in the set.
    pub dropped_on_load: usize,
}

/// Arguments of `remote.map`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MapArgs {
    /// Where the messages come from.
    pub source: RemoteSource,
    /// The control they drive.
    pub target: RemoteTarget,
    /// The control's kind. Left out, the control's own: a device parameter's
    /// comes from its description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ControlKind>,
    /// The bottom of the range, in the control's own units. Left out, the
    /// bottom of the control's range (R4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// The top of the range. Left out, the top of the control's range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// How a controller is read. Left out: Absolute, or Absolute (14-bit)
    /// for a 14-bit source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<RemoteMode>,
}

/// Which kind of mapping a control's mapping is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// The control's MIDI mapping.
    #[default]
    Midi,
    /// The control's key mapping.
    Key,
}

/// Arguments of `remote.unmap`: name the mapping by its `source`, or a
/// control by its `target` and the kind of mapping it has (`via`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnmapArgs {
    /// The mapping's source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RemoteSource>,
    /// The control whose mapping goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<RemoteTarget>,
    /// With `target`: its MIDI mapping or its key mapping. MIDI if left
    /// out.
    #[serde(default)]
    pub via: Via,
}

/// Arguments of `remote.set_range`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRangeArgs {
    /// The mapping's source.
    pub source: RemoteSource,
    /// The bottom of the range, in the control's own units.
    pub min: f64,
    /// The top of the range. Min above Max is allowed.
    pub max: f64,
}

/// Arguments of `remote.invert_range` and the mapping a command names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceArgs {
    /// The mapping's source.
    pub source: RemoteSource,
}

/// Arguments of `remote.set_mode`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetModeArgs {
    /// The mapping's source, a controller.
    pub source: RemoteSource,
    /// How its values are read.
    pub mode: RemoteMode,
}

/// What `remote.unmap` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnmapResult {
    /// How many mappings went.
    pub removed: usize,
}

/// Arguments of `remote.input`: a MIDI message by its `bytes`, or a computer
/// `key`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputArgs {
    /// One short MIDI message: a status byte and its data bytes, such as
    /// `[176, 20, 64]` for controller 20 at 64 on channel 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
    /// A computer key, named as the keymap names keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// With `key`: true presses it, false releases it. Left out, it is
    /// pressed and released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressed: Option<bool>,
}

/// What `remote.input` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputResult {
    /// Whether a mapping took the message or the key.
    pub taken: bool,
}

/// Which map mode is on (map modes R1, R2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MapMode {
    /// No map mode: mappings act.
    #[default]
    Off,
    /// MIDI Map Mode.
    Midi,
    /// Key Map Mode.
    Key,
}

/// Arguments of `remote.map_mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MapModeArgs {
    /// The mode to turn on, or `off`.
    pub mode: MapMode,
    /// With `midi` or `key`: turn the mode off when it is on already, as
    /// the switches and their keys do.
    #[serde(default)]
    pub toggle: bool,
}

/// Arguments of `remote.select`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectArgs {
    /// The control to select. Left out, the selection clears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<RemoteTarget>,
}

/// What `remote.map_mode`, `remote.select` and `remote.state` return.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateResult {
    /// The map mode that is on.
    pub mode: MapMode,
    /// The selected control, if one is.
    pub selected: Option<RemoteTarget>,
    /// The mode the last learned controller was guessed to be, while it is
    /// learned (R27 of remote mapping).
    pub guess: Option<RemoteMode>,
    /// Whether an input has its Remote switch on, so a message can reach
    /// learning.
    pub remote_input: bool,
    /// The Mapping Browser's rows while a map mode is on: the mode's
    /// mappings in order of their source. None otherwise.
    pub rows: Vec<MappingInfo>,
}

/// Adds the remote mapping commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(MAPPINGS, "MIDI and Key Mappings")
            .description(
                "Lists the set's MIDI and key mappings: each mapping's source (a note, a range \
                 of notes with its root, a controller, a 14-bit controller, pitch bend, or a \
                 computer key), the control it drives by IDs with its path and name, the \
                 control's kind, Min, Max and mode. The other remote commands name a mapping by \
                 the source reported here. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<MappingsResult>(),
    )?;
    registry.register(
        CommandSpec::new(MAP, "Map")
            .description(
                "Maps a source to a control, as a click on the control in a map mode and a \
                 message do. The control must be in the set. A source maps one control and a \
                 control has one MIDI mapping and one key mapping: mapping a source that maps \
                 another control moves it, and mapping a control that has a mapping of that \
                 kind replaces it. Min and Max start at the bottom and top of the control's \
                 range, a radio button's first and last options, or for a track activator or \
                 a device's on/off switch the controller values 64 and 127; a controller maps \
                 in Absolute mode (14-bit for a 14-bit source) unless `mode` says otherwise. \
                 `kind` is the control's own except for a device parameter's, which comes from \
                 its description. The mapping is saved with the set and the change is one undo \
                 step.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<MapArgs>()
            .result::<MappingInfo>(),
    )?;
    registry.register(
        CommandSpec::new(UNMAP, "Delete Mapping")
            .description(
                "Deletes a mapping: the one with `source`, or the MIDI or key mapping (`via`) \
                 of the control `target`. Deleting nothing is not an error and reports 0. One \
                 undo step.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<UnmapArgs>()
            .result::<UnmapResult>(),
    )?;
    registry.register(
        CommandSpec::new(SET_RANGE, "Set Mapping Range")
            .description(
                "Sets the Min and Max of the mapping with `source`, in the control's own \
                 units: a continuous control's value, a radio button's option numbers from 0, \
                 or a ranged switch's controller values from 0 to 127. Min above Max turns the \
                 control round. One undo step.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SetRangeArgs>()
            .result::<MappingInfo>(),
    )?;
    registry.register(
        CommandSpec::new(INVERT_RANGE, "Invert Range")
            .description(
                "Swaps the Min and Max of the mapping with `source`, as the Mapping Browser's \
                 Invert Range does. One undo step.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SourceArgs>()
            .result::<MappingInfo>(),
    )?;
    registry.register(
        CommandSpec::new(SET_MODE, "Set Mapping Mode")
            .description(
                "Sets how a controller mapping reads its values: `absolute`, `absolute14` for \
                 a 14-bit controller, or `relative` with an `encoding` (`signed_bit`, \
                 `signed_bit2`, `bin_offset` or `twos_complement`) and whether it is \
                 `accelerated`. Only a controller reads relative; a 14-bit controller reads \
                 only `absolute14`. One undo step.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SetModeArgs>()
            .result::<MappingInfo>(),
    )?;
    registry.register(
        CommandSpec::new(INPUT, "Remote Input")
            .description(
                "Applies a MIDI message (`bytes`) or a computer key (`key`, pressed and \
                 released unless `pressed` says which) as if it came from the remote input, so \
                 a mapping is tried without a controller. A message a mapping takes runs its \
                 control's command as a hand move does and never reaches a track; one no \
                 mapping takes does nothing here. The effect is the control's own and reads \
                 back as the control's command does.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<InputArgs>()
            .result::<InputResult>(),
    )?;
    registry.register(
        CommandSpec::new(MAP_MODE, "Map Mode")
            .description(
                "Turns MIDI Map Mode (`midi`) or Key Map Mode (`key`) on, or both off, as the \
                 Control Bar's switches, Cmd-M and Cmd-K do; with `toggle` a mode that is on \
                 already turns off. One map mode is on at a time: turning one on turns the \
                 other and any rack's Map mode off. While one is on every mappable control is \
                 tinted, a click selects instead of operating, no mapping acts, and the Mapping \
                 Browser lists the mode's mappings. View state: it isn't saved with the set and \
                 isn't an undo step, and it is off when a set opens. Read back in \
                 `remote.state`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding(
                Binding::new("S-MAP-001")
                    .arg("mode", "midi")
                    .arg("toggle", true),
            )
            .binding(
                Binding::new("S-MAP-002")
                    .arg("mode", "key")
                    .arg("toggle", true),
            )
            .args::<MapModeArgs>()
            .result::<StateResult>(),
    )?;
    registry.register(
        CommandSpec::new(SELECT, "Select Control to Map")
            .description(
                "Selects the control `target` names, as a click on its overlay does in a map \
                 mode, so the next note, controller or pitch bend on the remote input (MIDI Map \
                 Mode) or the next key (Key Map Mode) maps to it; with no `target` the \
                 selection clears, as Esc does. Needs a map mode on, and the control must be \
                 one the mode maps: Key Map Mode takes no continuous control. View state, \
                 read back in `remote.state`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<SelectArgs>()
            .result::<StateResult>(),
    )?;
    registry.register(
        CommandSpec::new(STATE, "Map Mode State")
            .description(
                "Reports the map mode that is on, the selected control, the mode guessed for \
                 the controller being learned, whether an input has its Remote switch on, and \
                 the Mapping Browser's rows: the mode's mappings in order of their source. \
                 Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<StateResult>(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");

        let schema = registry.get(MAP).unwrap().args.as_ref().unwrap();
        let args = json!({
            "source": { "type": "cc", "channel": 1, "controller": 20 },
            "target": { "type": "crossfader" },
            "min": 0.0, "max": 1.0
        });
        assert!(bl_commands::validate(schema, &args).is_ok());
        assert!(bl_commands::validate(schema, &json!({ "source": args["source"] })).is_err());
    }
}
