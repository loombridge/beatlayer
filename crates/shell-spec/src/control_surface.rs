//! The Control Surfaces table (WP-7.5; the control surfaces spec, R13 and
//! R14): `control_surface.list`, `control_surface.set`,
//! `control_surface.dump` and `control_surface.surfaces`, and the instant
//! mappings' commands (WP-7.6, R23 to R37): `control_surface.lock`,
//! `control_surface.unlock`, `control_surface.move_ring`,
//! `control_surface.banks` and `control_surface.set_bank`, with their
//! arguments and results.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them
//! by ID. The Settings window's table runs the same commands (R14).

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{DeviceId, ParamId, SceneId, TrackId};
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

/// `control_surface.lock`: lock a device to a row's surface.
pub const LOCK: &str = "control_surface.lock";
/// `control_surface.unlock`: give a row's surface back to the device in
/// focus.
pub const UNLOCK: &str = "control_surface.unlock";
/// `control_surface.move_ring`: move a row's session ring.
pub const MOVE_RING: &str = "control_surface.move_ring";
/// `control_surface.banks`: a device's banks, as a surface reads them.
pub const BANKS: &str = "control_surface.banks";
/// `control_surface.set_bank`: put a row's surface on a bank.
pub const SET_BANK: &str = "control_surface.set_bank";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    LIST, SET, DUMP, SURFACES, LOCK, UNLOCK, MOVE_RING, BANKS, SET_BANK,
];

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

/// Which ways a ring can move.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RingMoves {
    /// A track to the left.
    pub left: bool,
    /// A track to the right.
    pub right: bool,
    /// A scene up.
    pub up: bool,
    /// A scene down.
    pub down: bool,
}

/// Where a surface's session ring is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RingState {
    /// How many tracks wide.
    pub tracks: u8,
    /// How many scenes tall; 0 for a surface with no clip launching.
    pub scenes: u8,
    /// Its first track, by ID.
    pub first_track: Option<TrackId>,
    /// Its first scene, by ID.
    pub first_scene: Option<SceneId>,
    /// Its first track's place among the ring's tracks, from 0.
    pub track_index: usize,
    /// Its first scene's place among the scenes, from 0.
    pub scene_index: usize,
    /// Which ways it can move.
    pub can_move: RingMoves,
}

/// How a device is locked to a surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LockKind {
    /// By Lock to in the device's menu or `control_surface.lock`.
    Hand,
    /// By arming a MIDI track whose instrument the device is.
    Arming,
}

/// The device a surface's device controls reach.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceFocus {
    /// The device.
    pub device: DeviceId,
    /// How it is locked to the surface, if it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<LockKind>,
    /// The bank the surface shows, from 0.
    pub bank: usize,
    /// That bank's name.
    pub bank_name: String,
    /// Every bank's name, in order.
    pub banks: Vec<String>,
}

/// What a running surface reports of itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SurfaceStateInfo {
    /// Its session ring, if it has one.
    pub ring: Option<RingState>,
    /// The device its device controls reach, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceFocus>,
    /// Its own mode, by name.
    pub mode: Option<String>,
}

/// A surface a device is locked to: what `device_view.state` reports for a
/// device and its title bar's hand icon names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceLock {
    /// The row of the Control Surfaces table, from 1 to 6.
    pub row: u8,
    /// The surface's name.
    pub surface: String,
    /// How the device is locked to it.
    pub by: LockKind,
}

/// A running surface's ring as `session.state` reports it: the tracks and
/// scenes inside it, by ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RingBox {
    /// The row of the Control Surfaces table, from 1 to 6.
    pub row: u8,
    /// The surface's name.
    pub surface: String,
    /// The tracks it covers, left to right.
    pub tracks: Vec<TrackId>,
    /// The scenes it covers, top to bottom; none for a ring with no clip
    /// launching.
    pub scenes: Vec<SceneId>,
}

/// How `control_surface.move_ring` moves a ring by a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RingStep {
    /// One track left.
    Left,
    /// One track right.
    Right,
    /// One scene up.
    Up,
    /// One scene down.
    Down,
    /// Its width left.
    PageLeft,
    /// Its width right.
    PageRight,
    /// Its height up.
    PageUp,
    /// Its height down.
    PageDown,
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

/// Arguments of `control_surface.lock`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LockArgs {
    /// The row, from 1 to 6.
    pub row: u8,
    /// The device to lock.
    pub device: DeviceId,
}

/// Arguments of `control_surface.unlock`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnlockArgs {
    /// The row, from 1 to 6.
    pub row: u8,
}

/// Arguments of `control_surface.move_ring`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveRingArgs {
    /// The row, from 1 to 6.
    pub row: u8,
    /// The ring's new first track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The ring's new first scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneId>,
    /// A step instead of a place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<RingStep>,
}

/// Arguments of `control_surface.banks`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BanksArgs {
    /// The device.
    pub device: DeviceId,
}

/// One parameter of a bank, as a surface reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BankParam {
    /// The parameter.
    pub param: ParamId,
    /// Its name.
    pub name: String,
    /// Its value.
    pub value: f64,
    /// Its value as the device shows it.
    pub text: String,
    /// Whether a macro holds it, so a surface doesn't move it.
    pub held: bool,
}

/// One bank of a device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BankInfo {
    /// Its name.
    pub name: String,
    /// Its parameters, in the order a surface's device controls take them.
    pub params: Vec<BankParam>,
}

/// What `control_surface.banks` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BanksResult {
    /// The device.
    pub device: DeviceId,
    /// Its banks, in order.
    pub banks: Vec<BankInfo>,
}

/// Arguments of `control_surface.set_bank`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetBankArgs {
    /// The row, from 1 to 6.
    pub row: u8,
    /// The bank, from 0, as `control_surface.banks` lists them.
    pub bank: usize,
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
    registry.register(
        CommandSpec::new(LOCK, "Lock Device to Control Surface")
            .description(
                "Locks a device to a running control surface that supports locking, as Lock to \
                 in the device's menu does: the surface's device controls stay on that device \
                 whatever is selected. A surface locks to one device at a time, so locking \
                 another moves the lock; locking the device it is locked to gives it back. A \
                 device may be locked to several surfaces. The lock is surface state: it is \
                 never saved or an undo step, and ends when the device is deleted or the \
                 surface stops. Arming a MIDI track locks the surfaces to its instrument unless \
                 one was locked by hand. Read back by `control_surface.list` (`state.device`) \
                 and `device_view.state` (`locks`).",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<LockArgs>()
            .result::<RowInfo>(),
    )?;
    registry.register(
        CommandSpec::new(UNLOCK, "Unlock Device from Control Surface")
            .description(
                "Gives a row's surface back to the device in focus, ending the lock \
                 `control_surface.lock` or arming made. Changes surface state only. Read back \
                 by `control_surface.list`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<UnlockArgs>()
            .result::<RowInfo>(),
    )?;
    registry.register(
        CommandSpec::new(MOVE_RING, "Move Session Ring")
            .description(
                "Moves a row's session ring, the block of tracks and scenes its grid controls: \
                 to a first `track` and `scene` by ID, either or both, or by a `step` (a track \
                 or scene, or a page of its width or height). The ring stops where its last \
                 column holds the last track and its last row the last scene; with fewer \
                 tracks or scenes than its size it stays at the first. Its tracks are the \
                 Session View's columns, a folded group's tracks left out, then the return \
                 tracks. Selects nothing, launches nothing and adds no undo step; the Session \
                 View scrolls to show it. Read back by `control_surface.list` \
                 (`state.ring`) and `session.state` (`rings`).",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<MoveRingArgs>()
            .result::<RowInfo>(),
    )?;
    registry.register(
        CommandSpec::new(BANKS, "Device Banks")
            .description(
                "Lists a device's banks as a control surface reads them, each with its name \
                 and its parameters by ID, name and value text, in the order the surface's \
                 device controls take them (at most eight to a bank): a native device's own \
                 banks, a rack's macros, a plug-in's configured list in eights, and last the \
                 title bar's switch and the menu options. `held` marks a parameter a macro \
                 holds. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<BanksArgs>()
            .result::<BanksResult>(),
    )?;
    registry.register(
        CommandSpec::new(SET_BANK, "Set Control Surface Bank")
            .description(
                "Puts a running surface on a bank of the device its device controls reach, by \
                 index as `control_surface.banks` lists them, as the surface's own bank \
                 buttons do. The surface shows that bank of that device again whenever it \
                 comes back to it while it runs. Changes surface state only. Read back by \
                 `control_surface.list` (`state.device`).",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<SetBankArgs>()
            .result::<RowInfo>(),
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
