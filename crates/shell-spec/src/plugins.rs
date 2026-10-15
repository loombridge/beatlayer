//! Plug-in commands in the command registry (WP-2.6; the CLAP hosting
//! spec, and WP-2.7 for VST3): loading a plug-in from its file as a
//! device, its editor window, and what it's doing.
//!
//! A plug-in device is otherwise a device like any other: `device.set_param`
//! changes its parameters, `device.parameters` lists them, and saving the
//! set keeps its state. Finding plug-ins by scanning the system's plug-in
//! folders is WP-2.9's; an editor embeds into a window of Beatlayer's own
//! (WP-2.10) on Windows and Linux, where the host builds one.

use bl_commands::Context::{DeviceView, Global};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::{DeviceId, DeviceRole, ParamId, PluginFormat, TrackId, WindowFrame};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `plugin.load`: load a plug-in from its file and add it as a device.
pub const LOAD: &str = "plugin.load";
/// `plugin.load_au`: load an Audio Unit, by its component description, and
/// add it as a device (WP-2.8, macOS only).
pub const LOAD_AU: &str = "plugin.load_au";
/// `plugin.show_window`: open or close a plug-in device's editor window.
pub const SHOW_WINDOW: &str = "plugin.show_window";
/// `plugin.info`: what a plug-in device's plug-in is doing.
pub const INFO: &str = "plugin.info";
/// `plugin.set_window`: where a plug-in device's editor window sits
/// (WP-2.10).
pub const SET_WINDOW: &str = "plugin.set_window";
/// `plugin.set_panel_param`: which parameter a plug-in device's panel knob
/// shows (WP-2.10, D19).
pub const SET_PANEL_PARAM: &str = "plugin.set_panel_param";
/// `plugin.scan`: find the plug-ins installed on this machine (WP-2.9).
pub const SCAN: &str = "plugin.scan";
/// `plugin.rescan`: scan again, ignoring the cache (WP-2.9).
pub const RESCAN: &str = "plugin.rescan";
/// `plugin.list`: every plug-in the set uses, and where (WP-2.9, IMP-29).
pub const LIST: &str = "plugin.list";
/// `plugin.settings`: hidden plug-ins and the format preference (WP-2.9,
/// IMP-07).
pub const SETTINGS: &str = "plugin.settings";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    LOAD,
    LOAD_AU,
    SHOW_WINDOW,
    INFO,
    SET_WINDOW,
    SET_PANEL_PARAM,
    SCAN,
    RESCAN,
    LIST,
    SETTINGS,
];

/// How many knobs a plug-in device's panel shows (WP-2.10, D19, IMP-03):
/// its first parameters'.
pub const PANEL_KNOBS: usize = 8;

/// Whether `id` is one of the plug-in commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `plugin.load`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadPluginArgs {
    /// The plug-in's file or bundle (`.clap`, or `.vst3` for VST3).
    pub path: String,
    /// Which of the file's plug-ins, by its ID; the first if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// The track; the selected track if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// Where it goes in the chain; as `device.add` places it if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// Arguments of `plugin.load_au`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadAudioUnitArgs {
    /// The component's type, four characters: `aufx` for an effect, `aumu`
    /// or `augn` for an instrument, `aumi` for a MIDI effect.
    #[serde(rename = "type")]
    pub component_type: String,
    /// The component's subtype, four characters, such as `dely` for Apple's
    /// AUDelay.
    pub subtype: String,
    /// The component's manufacturer, four characters, such as `appl` for
    /// Apple's units.
    pub manufacturer: String,
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
    /// The plug-in's ID: a CLAP ID, or a VST3 class ID.
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
    /// The plug-in's ID: a CLAP ID, or a VST3 class ID.
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
    /// Whether it has an editor that can open: embedded in a window of
    /// Beatlayer's own on Windows and Linux (WP-2.10), or in a floating
    /// window of the plug-in's own.
    pub has_window: bool,
    /// Whether its window is open.
    pub window_open: bool,
    /// Whether its window is shown: open, and not hidden by View ▸
    /// Plug-In Windows or Auto-Hide Plug-In Windows. Hiding is not
    /// closing: a hidden window is still open.
    pub window_shown: bool,
    /// Where its window sits and how big it is, once it has been placed
    /// (`plugin.set_window`); `null` until then. Saved with the set beside
    /// the document, so moving or resizing is never an undo step
    /// (WP-2.10; as [`FloatingWindow`](crate::devices::FloatingWindow)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowFrame>,
    /// Each parameter's value, as the plug-in itself reports it, as
    /// `[parameter, value]` pairs.
    pub values: Vec<(ParamId, f64)>,
}

/// Arguments of `plugin.set_window`: where a plug-in device's editor
/// window sits and how big it is (WP-2.10). Not an undo step.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetWindowArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// Where its window sits and how big it is, in points.
    pub frame: WindowFrame,
}

/// What `plugin.set_window` returns: the window's place, as `plugin.info`
/// reports it.
pub type SetWindowResult = PluginInfo;

/// Arguments of `plugin.set_panel_param`: which parameter one of a plug-in
/// device's panel knobs shows (WP-2.10, D19). The mapping is remembered
/// per plug-in, so another device of the same plug-in keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPanelParamArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The knob, 0 for the first (leftmost) and up to
    /// [`PANEL_KNOBS`] − 1.
    pub index: usize,
    /// The parameter it shows, from the plug-in's own; `null` restores the
    /// knob to the plug-in's parameter at that place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<ParamId>,
}

/// What `plugin.set_panel_param` returns: the device's panel, as
/// `device_view.state` reports it.
pub type SetPanelParamResult = crate::devices::DeviceViewState;

/// Arguments of `plugin.scan` and `plugin.rescan`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScanArgs {
    /// The files or folders to scan; the system's plug-in folders if left
    /// out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
}

/// A plug-in the scanner found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScannedItem {
    /// Its format (`clap`, `vst3`, `au`, or `lv2`).
    pub format: PluginFormat,
    /// The file or bundle it's in.
    pub path: String,
    /// Its identifier within its format (a CLAP ID, or a VST3 class ID).
    pub plugin_id: String,
    /// Its name.
    pub name: String,
    /// Its vendor, if it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Its version, if it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// What it is in a chain.
    pub role: DeviceRole,
    /// Whether the browser hides it (IMP-07).
    pub hidden: bool,
}

/// A plug-in that failed a scan, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BlockedItem {
    /// Its format.
    pub format: PluginFormat,
    /// The file or bundle it's in.
    pub path: String,
    /// Why it failed: it crashed, hung, or refused to load.
    pub reason: String,
}

/// What `plugin.scan` and `plugin.rescan` return.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginScan {
    /// Every plug-in the scan found.
    pub plugins: Vec<ScannedItem>,
    /// Every plug-in that failed, with why.
    pub blocked: Vec<BlockedItem>,
    /// How many files the scanner loaded.
    pub scanned: usize,
    /// How many files the cache answered.
    pub cached: usize,
}

/// Args of `plugin.list`: none.
pub type ListArgs = bl_commands::NoArgs;

/// One plug-in a set uses (IMP-29).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SetPlugin {
    /// Its format.
    pub format: PluginFormat,
    /// Its identifier within its format.
    pub plugin_id: String,
    /// Its name, as the set's device has it.
    pub name: String,
    /// The file the set says it came from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Its vendor, from the scanner, if it's known on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Its version, from the scanner, if it's known on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The devices in the set that use it.
    pub devices: Vec<DeviceId>,
    /// The tracks those devices are on.
    pub tracks: Vec<TrackId>,
    /// Whether this machine has it: the scanner hasn't seen it, or the
    /// scanner blocklisted its file.
    pub missing: bool,
    /// Why it's missing, if the scanner blocklisted its file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

/// What `plugin.list` returns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SetPluginList {
    /// Every plug-in the set uses.
    pub plugins: Vec<SetPlugin>,
}

/// Arguments of `plugin.settings`: the fields to change; none to read them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginSettingsArgs {
    /// The hidden plug-ins to remember, as `format/plugin_id` keys (IMP-07);
    /// empty hides none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<Vec<String>>,
    /// Whether the browser shows hidden plug-ins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_hidden: Option<bool>,
    /// The format to prefer when a plug-in exists in several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_preference: Option<PluginFormat>,
}

/// What `plugin.settings` returns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PluginSettings {
    /// The hidden plug-ins, as `format/plugin_id` keys.
    pub hidden: Vec<String>,
    /// Whether the browser shows hidden plug-ins.
    pub show_hidden: bool,
    /// The preferred format, if one is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_preference: Option<PluginFormat>,
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
                "Loads a plug-in from its file or bundle (`path`: a `.clap` file, or a \
                 `.vst3` bundle or module) and adds it as a device, as `device.add` adds one: \
                 to the selected track (or `track`), at `index` or where its role goes, \
                 selected, one undo step (Insert Device). `plugin` picks one of the file's \
                 plug-ins by its ID (CLAP's, or a VST3 class ID); the first if left out. Its \
                 parameters become the device's, which `device.parameters` lists and \
                 `device.set_param` and automation change. The plug-in's code runs inside \
                 Beatlayer, so load only plug-ins you trust. The scanner (`plugin.scan`) \
                 lists the plug-ins on this machine; this command still loads one by its \
                 path.",
            )
            .permission(Permission::External)
            .contexts([Global, DeviceView])
            .args::<LoadPluginArgs>()
            .result::<PluginLoaded>(),
    )?;
    registry.register(
        CommandSpec::new(LOAD_AU, "Load Audio Unit")
            .description(
                "Loads the Audio Unit with the given component description — its `type`, \
                 `subtype`, and `manufacturer`, four characters each, such as Apple's \
                 `aufx`, `dely`, `appl` for AUDelay — and adds it as a device, as \
                 `device.add` adds one: to the selected track (or `track`), at `index` or \
                 where its role goes, selected, one undo step (Insert Device). Its \
                 parameters become the device's, which `device.parameters` lists and \
                 `device.set_param` and automation change; its own state is saved with the \
                 set. Audio Units are macOS only: on Windows and Linux this fails with \
                 that reason. Its own editor window arrives with the Audio Unit scanner; \
                 `plugin.show_window` says so until then. The unit's code runs inside \
                 Beatlayer, so load only units you trust.",
            )
            .permission(Permission::External)
            .contexts([Global, DeviceView])
            .args::<LoadAudioUnitArgs>()
            .result::<PluginLoaded>(),
    )?;
    registry.register(
        CommandSpec::new(SHOW_WINDOW, "Show Plug-In Window")
            .description(
                "Opens or closes a plug-in device's editor, as the device's title bar button \
                 does; switches unless `shown` says which. On Windows and Linux the editor \
                 opens in a window Beatlayer owns, with the plug-in's own view embedded into \
                 it; on macOS, and for a plug-in that won't embed, in a floating window of the \
                 plug-in's own. An open window follows View ▸ Plug-In Windows and Auto-Hide \
                 Plug-In Windows, which hide and show it without closing it. It changes only \
                 the view: it isn't an undo step. Returns `plugin.info`'s report.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<ShowWindowArgs>()
            .result::<PluginInfo>(),
    )?;
    registry.register(
        CommandSpec::new(INFO, "Plug-In Info")
            .description(
                "Reports what a plug-in device's plug-in is doing: its ID and file, whether \
                 it's running, the latency it reports in frames, what went wrong if anything, \
                 whether its editor window can open, is open, and is shown (a hidden window \
                 stays open), and its parameters' values as it reports them. Changes \
                 nothing.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<PluginInfoArgs>()
            .result::<PluginInfo>(),
    )?;
    registry.register(
        CommandSpec::new(SET_WINDOW, "Set Plug-In Window Place")
            .description(
                "Records where a plug-in device's editor window sits and how big it is, in \
                 points (`frame`): its place is saved with the set beside the document, so \
                 moving or resizing a window is never an undo step. A plug-in window reopens \
                 where it was left. Returns `plugin.info`'s report.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<SetWindowArgs>()
            .result::<SetWindowResult>(),
    )?;
    registry.register(
        CommandSpec::new(SET_PANEL_PARAM, "Set Plug-In Panel Parameter")
            .description(
                "Sets which of a plug-in device's own parameters a panel knob shows (D19): \
                 `index` is the knob, 0 for the first and up to seven, and `param` the \
                 parameter, or null to restore the knob to the plug-in's parameter at that \
                 place. The mapping is remembered per plug-in, so another device of the same \
                 plug-in keeps it, and it changes only the view: it isn't an undo step. \
                 Returns the Device View's state.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<SetPanelParamArgs>()
            .result::<SetPanelParamResult>(),
    )?;
    registry.register(
        CommandSpec::new(SCAN, "Scan Plug-Ins")
            .description(
                "Finds the plug-ins installed on this machine and describes each one: its \
                 format, its file, its identifier, name, vendor, version, and role in a chain. \
                 Scans the files or folders in `paths`, or the system's plug-in folders when \
                 `paths` is left out. Each plug-in is loaded in a separate process with a \
                 timeout, so a plug-in that crashes or hangs is put on a blocklist with the \
                 reason instead of taking Beatlayer down; a file whose size and modification \
                 time are unchanged is answered from the cache. CLAP is the only format \
                 scanned so far. Reads plug-in files outside the project, so an agent asks \
                 first.",
            )
            .permission(Permission::External)
            .contexts([Global, DeviceView])
            .args::<ScanArgs>()
            .result::<PluginScan>(),
    )?;
    registry.register(
        CommandSpec::new(RESCAN, "Rescan Plug-Ins")
            .description(
                "Scans the plug-ins in `paths` (or the system's plug-in folders) again, \
                 ignoring the cache and clearing the blocklist for them, as the Rescan button \
                 does. Returns `plugin.scan`'s report.",
            )
            .permission(Permission::External)
            .contexts([Global, DeviceView])
            .args::<ScanArgs>()
            .result::<PluginScan>(),
    )?;
    registry.register(
        CommandSpec::new(LIST, "List Plug-Ins in the Set")
            .description(
                "Lists every plug-in the set uses, grouped by plug-in: its format, its \
                 identifier, name, the file the set says it came from, its vendor and version \
                 when the scanner knows them, the devices and tracks that use it, and whether \
                 this machine has it (missing when the scanner hasn't seen it, or says why it \
                 is blocklisted). Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<ListArgs>()
            .result::<SetPluginList>(),
    )?;
    registry.register(
        CommandSpec::new(SETTINGS, "Plug-In Settings")
            .description(
                "Reads or changes how the browser shows plug-ins (IMP-07): `hidden` is the \
                 plug-ins to hide, as `format/plugin_id` keys; `show_hidden` whether hidden \
                 plug-ins still show; and `format_preference` the format to prefer when a \
                 plug-in exists in several. Fields left out are unchanged. Returns the \
                 settings. A set that uses a hidden plug-in still loads it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<PluginSettingsArgs>()
            .result::<PluginSettings>(),
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
