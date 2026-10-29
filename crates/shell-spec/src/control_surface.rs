//! The Control Surfaces table (WP-7.5; the control surfaces spec, R13 and
//! R14): `control_surface.list`, `control_surface.set`,
//! `control_surface.dump` and `control_surface.surfaces`, with their
//! arguments and results.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them
//! by ID. The Settings window's table runs the same commands (R14).

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `control_surface.list`: the six rows of the Control Surfaces table.
pub const LIST: &str = "control_surface.list";
/// `control_surface.set`: set a row's surface, input and output.
pub const SET: &str = "control_surface.set";
/// `control_surface.dump`: send a row's surface's setup to its output.
pub const DUMP: &str = "control_surface.dump";
/// `control_surface.surfaces`: the surfaces the table's chooser offers.
pub const SURFACES: &str = "control_surface.surfaces";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[LIST, SET, DUMP, SURFACES];

/// Whether `id` is one of the control surface commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Reads a field that may be given as `null`, which is different from leaving
/// it out: `null` is `Some(None)`.
fn given<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// What a row's surface is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RowRun {
    /// The row names no surface.
    Empty,
    /// The surface waits for a port to appear.
    Waiting,
    /// The surface runs.
    Running,
    /// The surface is stopped; `error` says why.
    Stopped,
}

/// The surface a row names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SurfaceRef {
    /// Its ID.
    pub id: String,
    /// Its name, if Beatlayer offers a surface with that ID.
    pub name: Option<String>,
}

/// A port a row names, and whether the MIDI system offers it now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortRef {
    /// The name the MIDI system gives the port.
    pub name: String,
    /// Whether the port is present now. A missing port keeps its name, and
    /// the surface starts when a port of that name appears.
    pub present: bool,
}

/// The size of a surface's ring: the block of tracks and scenes its grid
/// controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RingInfo {
    /// How many tracks wide.
    pub tracks: u8,
    /// How many scenes tall; 0 for a surface with no clip launching.
    pub scenes: u8,
}

/// What a running surface reports of itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SurfaceStateInfo {
    /// Its ring, if it has one.
    pub ring: Option<RingInfo>,
    /// Its own mode, by name.
    pub mode: Option<String>,
}

/// One row of the table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RowInfo {
    /// The row, from 1 to 6.
    pub row: u8,
    /// The surface, or none.
    pub surface: Option<SurfaceRef>,
    /// The input, or none.
    pub input: Option<PortRef>,
    /// The output, or none.
    pub output: Option<PortRef>,
    /// Whether the surface runs, waits for a port, or stopped.
    pub state: RowRun,
    /// Why the surface stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether Dump is enabled: the surface has a setup to send and its
    /// output is present.
    pub can_dump: bool,
    /// What a running surface reports of itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_state: Option<SurfaceStateInfo>,
}

/// What `control_surface.list` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListResult {
    /// The six rows, in order.
    pub rows: Vec<RowInfo>,
}

/// One surface the chooser offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SurfaceInfo {
    /// Its ID, which `control_surface.set` takes.
    pub id: String,
    /// Its name, as the chooser shows it.
    pub name: String,
    /// Whether it needs an output to start.
    pub needs_output: bool,
    /// Whether a device can be locked to it.
    pub can_lock: bool,
    /// The size of its ring, if it has one.
    pub ring: Option<RingInfo>,
    /// Whether it has a setup for Dump to send.
    pub has_setup: bool,
}

/// What `control_surface.surfaces` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SurfacesResult {
    /// Every surface the chooser offers, native ones by name.
    pub surfaces: Vec<SurfaceInfo>,
}

/// Arguments of `control_surface.set`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    /// The row, from 1 to 6.
    pub row: u8,
    /// The surface's ID from `control_surface.surfaces`, or `null` for None.
    /// Left out, it stays as it is; given again, a surface that stopped with
    /// an error starts again.
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub surface: Option<Option<String>>,
    /// The input's name from `midi.ports`, or `null` for None. A port that
    /// isn't present is kept by name. Left out, it stays as it is.
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub input: Option<Option<String>>,
    /// The output's name, or `null` for None. Left out, it stays as it is.
    #[serde(
        default,
        deserialize_with = "given",
        skip_serializing_if = "Option::is_none"
    )]
    pub output: Option<Option<String>>,
}

/// Arguments of `control_surface.dump`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DumpArgs {
    /// The row, from 1 to 6.
    pub row: u8,
}

/// What `control_surface.dump` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DumpResult {
    /// How many messages were sent to the output.
    pub sent: usize,
}

/// Adds the control surface commands' specs to `registry`.
///
/// # Errors
///
/// [`RegisterError`] if an ID is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(LIST, "List Control Surfaces")
            .description(
                "Lists the six rows of the Control Surfaces table (Settings, Tempo & MIDI): \
                 each row's surface by ID and name, its input and output by name with whether \
                 each is present, whether the surface runs, waits for a port to appear or is \
                 stopped with an error (`state` and `error`), whether Dump is enabled, and, \
                 for a running surface, what it reports of itself: its ring and its own mode. \
                 Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<ListResult>(),
    )?;
    registry.register(
        CommandSpec::new(SET, "Set Control Surface")
            .description(
                "Sets a row of the Control Surfaces table, as its choosers do: the surface by \
                 ID (`control_surface.surfaces`), the input and the output by name \
                 (`midi.ports`), any of them; `null` is None, and what is left out stays. A \
                 port a row names is open while the row names it, whatever its Track, Sync \
                 and Remote switches say, and a port serves one row: naming one another row \
                 uses is refused, naming that row. A port that isn't present is kept by name \
                 and the surface starts when it appears. The surface starts when its row has \
                 it and its input is present, and its output where it needs one; it stops, \
                 sending the controller its goodbye, when the row changes, its input goes away \
                 or Beatlayer quits. Choosing a surface again restarts one that stopped with \
                 an error. The rows are kept for the next start, and are not part of the set \
                 or an undo step. Opens and closes devices outside Beatlayer and sends them \
                 messages.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<SetArgs>()
            .result::<RowInfo>(),
    )?;
    registry.register(
        CommandSpec::new(DUMP, "Dump Control Surface Setup")
            .description(
                "Sends a row's surface's setup to the row's output, as the row's Dump button \
                 does. It works while the surface declares a setup to send and the output is \
                 present (`can_dump` in `control_surface.list`), and fails otherwise. Changes \
                 nothing in the set. Sends messages to a device outside Beatlayer.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<DumpArgs>()
            .result::<DumpResult>(),
    )?;
    registry.register(
        CommandSpec::new(SURFACES, "Control Surfaces")
            .description(
                "Lists every control surface the table's chooser offers, with the ID \
                 `control_surface.set` takes, whether it needs an output, whether a device can \
                 be locked to it, the size of its ring and whether it has a setup for Dump. \
                 Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<SurfacesResult>(),
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

        let schema = registry.get(SET).unwrap().args.as_ref().unwrap();
        assert!(
            bl_commands::validate(
                schema,
                &json!({ "row": 1, "surface": null, "input": "IAC" })
            )
            .is_ok()
        );
        assert!(bl_commands::validate(schema, &json!({ "surface": "test" })).is_err());
    }

    #[test]
    fn null_is_none_and_a_missing_field_is_left_alone() {
        let args: SetArgs =
            serde_json::from_value(json!({ "row": 2, "surface": null, "output": "Out" })).unwrap();
        assert_eq!(
            (args.surface, args.input, args.output),
            (Some(None), None, Some(Some("Out".to_owned())))
        );
    }
}
