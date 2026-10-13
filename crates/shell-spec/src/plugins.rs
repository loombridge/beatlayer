//! Plug-in commands in the command registry (WP-2.6; the CLAP hosting
//! spec): loading a CLAP plug-in from its file as a device, its editor
//! window, and what it's doing.
//!
//! A plug-in device is otherwise a device like any other: `device.set_param`
//! changes its parameters, `device.parameters` lists them, and saving the
//! set keeps its state. Finding plug-ins by scanning the system's plug-in
//! folders is WP-2.9's; embedding editors in Beatlayer's own windows is
//! WP-2.10's.

use bl_commands::Context::{DeviceView, Global};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::{DeviceId, ParamId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `plugin.load`: load a CLAP plug-in from its file and add it as a device.
pub const LOAD: &str = "plugin.load";
/// `plugin.show_window`: open or close a plug-in device's editor window.
pub const SHOW_WINDOW: &str = "plugin.show_window";
/// `plugin.info`: what a plug-in device's plug-in is doing.
pub const INFO: &str = "plugin.info";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[LOAD, SHOW_WINDOW, INFO];

/// Whether `id` is one of the plug-in commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `plugin.load`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadPluginArgs {
    /// The CLAP plug-in's file or bundle (`.clap`).
    pub path: String,
    /// Which of the file's plug-ins, by its CLAP ID; the first if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// The track; the selected track if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// Where it goes in the chain; as `device.add` places it if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// What `plugin.load` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PluginLoaded {
    /// The new device, now selected.
    pub device: DeviceId,
    /// Its track.
    pub track: TrackId,
    /// Whether a new MIDI track was made for it, as `device.add` does for
    /// an instrument or MIDI effect.
    pub new_track: bool,
    /// The plug-in's CLAP ID.
    pub plugin: String,
    /// Its name.
    pub name: String,
}

/// Arguments of `plugin.show_window`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowWindowArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// Open (true) or close (false) the window; switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<bool>,
}

/// Arguments of `plugin.info`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginInfoArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
}

/// What `plugin.info` and `plugin.show_window` return.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginInfo {
    /// The device.
    pub device: DeviceId,
    /// The plug-in's CLAP ID.
    pub plugin: String,
    /// The file it was loaded from.
    pub path: String,
    /// Whether it's running: activated and processing for the engine.
    pub active: bool,
    /// The latency it reports, in frames, as of its last activation. Delay
    /// compensation (WP-2.11) lines other tracks up with it.
    pub latency: u32,
    /// What went wrong, if anything: it couldn't be found or loaded,
    /// refused to activate (the device passes its input through), or
    /// failed while processing (the device is silent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// Whether it has an editor that opens in a window of its own.
    pub has_window: bool,
    /// Whether its window is open.
    pub window_open: bool,
    /// Whether its window is shown: open, and not hidden by View ▸
    /// Plug-In Windows or Auto-Hide Plug-In Windows. Hiding is not
    /// closing: a hidden window is still open.
    pub window_shown: bool,
    /// Each parameter's value, as the plug-in itself reports it, as
    /// `[parameter, value]` pairs.
    pub values: Vec<(ParamId, f64)>,
}

/// Adds the plug-in commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(LOAD, "Load Plug-In")
            .description(
                "Loads a CLAP plug-in from its file or bundle (`path`, a `.clap`) and adds it \
                 as a device, as `device.add` adds one: to the selected track (or `track`), at \
                 `index` or where its role goes, selected, one undo step (Insert Device). \
                 `plugin` picks one of the file's plug-ins by its CLAP ID; the first if left \
                 out. Its parameters become the device's, which `device.parameters` lists and \
                 `device.set_param` and automation change; its own state is saved with the \
                 set. The plug-in's code runs inside Beatlayer, so load only plug-ins you \
                 trust. Until the plug-in scanner arrives, this is how plug-ins are loaded.",
            )
            .permission(Permission::External)
            .contexts([Global, DeviceView])
            .args::<LoadPluginArgs>()
            .result::<PluginLoaded>(),
    )?;
    registry.register(
        CommandSpec::new(SHOW_WINDOW, "Show Plug-In Window")
            .description(
                "Opens or closes a plug-in device's own editor in a window of its own, as the \
                 device's title bar button does; switches unless `shown` says which. Only \
                 plug-ins whose editor opens in its own window can show one yet. An open \
                 window follows View ▸ Plug-In Windows and Auto-Hide Plug-In Windows, which \
                 hide and show it without closing it. It changes only the view: it isn't an \
                 undo step. Returns `plugin.info`'s report.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<ShowWindowArgs>()
            .result::<PluginInfo>(),
    )?;
    registry.register(
        CommandSpec::new(INFO, "Plug-In Info")
            .description(
                "Reports what a plug-in device's plug-in is doing: its CLAP ID and file, \
                 whether it's running, the latency it reports in frames, what went wrong if \
                 anything, whether its editor window can open, is open, and is shown (a \
                 hidden window stays open), and its parameters' values as it reports them. \
                 Changes nothing.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<PluginInfoArgs>()
            .result::<PluginInfo>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for id in IDS {
            let spec = registry.get(id).unwrap_or_else(|| panic!("{id}"));
            assert!(spec.permission.is_some(), "{id}");
            assert!(spec.args.is_some() && spec.result.is_some(), "{id}");
            assert!(!spec.description.is_empty(), "{id}");
        }
        assert_eq!(registry.iter().count(), IDS.len());
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
    }
}
