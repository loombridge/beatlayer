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
use bl_model::{
    DeviceId, DeviceRole, MpeSettings, ParamId, PluginFormat, PluginMpe, TrackId, WindowFrame,
};
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
/// `plugin.configure`: the Configure switch on a plug-in device, which opens
/// and closes Plug-In Configure Mode (WP-5.40, manual 24.1.2).
pub const CONFIGURE: &str = "plugin.configure";
/// `plugin.configure_param`: adds a parameter to a plug-in device's
/// configured list (WP-5.40, manual 24.1.2).
pub const CONFIGURE_PARAM: &str = "plugin.configure_param";
/// `plugin.unconfigure_param`: removes one from the configured list, warning
/// first when it has automation, clip envelopes, or mappings (WP-5.40).
pub const UNCONFIGURE_PARAM: &str = "plugin.unconfigure_param";
/// `plugin.move_configured_param`: reorders the configured list (WP-5.40).
pub const MOVE_CONFIGURED_PARAM: &str = "plugin.move_configured_param";
/// `plugin.show_mpe_settings`: opens or closes the MPE/Multi-channel
/// Settings dialog (the MPE spec, R18).
pub const SHOW_MPE_SETTINGS: &str = "plugin.show_mpe_settings";
/// `plugin.scan`: find the plug-ins installed on this machine (WP-2.9).
pub const SCAN: &str = "plugin.scan";
/// `plugin.rescan`: scan again, ignoring the cache (WP-2.9).
pub const RESCAN: &str = "plugin.rescan";
/// `plugin.list`: every plug-in the set uses, and where (WP-2.9, IMP-29).
pub const LIST: &str = "plugin.list";
/// `plugin.export_list`: write the set's plug-in list to a text file (IMP-29,
/// WP-6.41).
pub const EXPORT_LIST: &str = "plugin.export_list";
/// `plugin.settings`: hidden plug-ins and the format preference (WP-2.9,
/// IMP-07).
pub const SETTINGS: &str = "plugin.settings";
/// `ara.state`: what each ARA plug-in's document holds and has done (IMP-20,
/// the ARA hosting spec).
pub const ARA_STATE: &str = "ara.state";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    LOAD,
    LOAD_AU,
    SHOW_WINDOW,
    INFO,
    SET_WINDOW,
    SET_PANEL_PARAM,
    CONFIGURE,
    CONFIGURE_PARAM,
    UNCONFIGURE_PARAM,
    MOVE_CONFIGURED_PARAM,
    SHOW_MPE_SETTINGS,
    SCAN,
    RESCAN,
    LIST,
    EXPORT_LIST,
    SETTINGS,
    ARA_STATE,
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
    /// The plug-in's file or bundle (`.clap`, `.vst3` for VST3, or an LV2
    /// bundle folder, `.lv2`, on Linux).
    pub path: String,
    /// Which of the file's plug-ins, by its ID (an LV2 plug-in's URI); the
    /// first if left out.
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
    /// The plug-in's ID: a CLAP ID, a VST3 class ID, or an LV2 URI.
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
    /// The plug-in's ID: a CLAP ID, a VST3 class ID, or an LV2 URI.
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
    /// What the device is as an ARA plug-in, if it is one: the track it
    /// renders (`null` while it renders none) and how many playback regions
    /// its document holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ara: Option<AraDevice>,
    /// Its MPE Mode and the MPE/Multi-channel Settings its MPE MIDI goes
    /// out on (the MPE spec, R14, R18).
    #[serde(default)]
    pub mpe: PluginMpe,
}

/// Arguments of `plugin.show_mpe_settings`: where the dialog is for. Give a
/// `device` (a plug-in's, or an External Instrument's, whose MIDI To is set
/// to MPE) or a `track` (a MIDI track whose MIDI To is a port set to MPE).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowMpeSettingsArgs {
    /// The plug-in or External Instrument device; the selected device if
    /// neither this nor `track` is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// A MIDI track, for its MIDI To.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// Open (true) or close (false) the dialog; opens if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<bool>,
}

/// What `plugin.show_mpe_settings` returns: whether the dialog is open and
/// what it edits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MpeSettingsDialog {
    /// Whether the dialog is open.
    pub open: bool,
    /// The plug-in or External Instrument device it edits, if it edits one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The track whose MIDI To it edits, if it edits one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The settings it shows, while it is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<MpeSettings>,
}

/// An ARA plug-in device, as `plugin.info` reports it (IMP-20).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AraDevice {
    /// The plug-in's ARA factory ID.
    pub factory: String,
    /// The track whose clips it renders: the one it is the ARA device of.
    pub track: Option<TrackId>,
    /// The playback regions its document holds, on every track.
    pub regions: usize,
}

/// Arguments of `ara.state`: none.
pub type AraStateArgs = bl_commands::NoArgs;

/// What `ara.state` returns: every ARA plug-in's document, in the order of
/// its factory ID.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AraState {
    /// The ARA plug-ins in the set that are running.
    pub plugins: Vec<AraPlugin>,
}

/// One ARA plug-in's document (IMP-20).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AraPlugin {
    /// The plug-in's ARA factory ID.
    pub factory: String,
    /// Its name.
    pub name: String,
    /// Whether it plays a region at another speed than its audio's, so
    /// warped clips go to it too.
    pub timestretch: bool,
    /// The devices bound to the document, each with the track it renders.
    pub devices: Vec<AraBoundDevice>,
    /// How many region sequences (tracks), audio sources (files) and audio
    /// modifications (clips) it holds.
    pub sequences: usize,
    pub sources: usize,
    pub modifications: usize,
    /// Its playback regions.
    pub regions: Vec<AraRegionInfo>,
    /// How many edit cycles the set has been given to it in.
    pub edit_cycles: u64,
    /// How many audio readers it opened, how many reads it made, and how
    /// many frames it read.
    pub readers_opened: u64,
    pub reader_calls: u64,
    pub frames_read: u64,
    /// Reads that came from inside its render call: always 0, because an
    /// audio reader never runs on the audio thread.
    pub audio_thread_reads: u64,
    /// Files it couldn't be given because they couldn't be read.
    pub missing: Vec<bl_model::ContentHash>,
    /// Whether it restored its archive when it was made: `restored`, or why
    /// it couldn't; `null` if the set held none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored: Option<String>,
}

/// A device bound to an ARA document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AraBoundDevice {
    /// The device.
    pub device: DeviceId,
    /// The track it renders, if any.
    pub track: Option<TrackId>,
}

/// One playback region of an ARA plug-in: a straight stretch of a clip's
/// audio put on the timeline. Times are seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AraRegionInfo {
    /// The clip, and which stretch of it: a warped clip has a piece for each
    /// warp marker and tempo change.
    pub clip: bl_model::ClipId,
    pub piece: u32,
    /// The track.
    pub track: TrackId,
    /// Where in the clip's file the region starts, and how much of it plays.
    pub start_in_audio: f64,
    pub duration_in_audio: f64,
    /// Where on the song's timeline it starts, and how long it lasts.
    pub start: f64,
    pub duration: f64,
    /// Whether the audio plays faster or slower than it was recorded.
    pub stretched: bool,
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

/// Arguments of `plugin.configure`: the Configure switch (WP-5.40, manual
/// 24.1.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// Turn configure mode on (true) or off (false); switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// What `plugin.configure` returns: the Device View's state, whose device's
/// `configure` says whether the switch is on.
pub type ConfigureResult = crate::devices::DeviceViewState;

/// Arguments of `plugin.configure_param`: adds one of the plug-in's
/// parameters to the device's configured list (WP-5.40, manual 24.1.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureParamArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The parameter to add.
    pub param: ParamId,
    /// Where it goes in the list; at the end if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// Arguments of `plugin.unconfigure_param`: removes one parameter from the
/// device's configured list (WP-5.40, manual 24.1.2). One that has
/// automation, a clip envelope, or a mapping is refused unless `force` says
/// to remove it anyway.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnconfigureParamArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The parameter to remove.
    pub param: ParamId,
    /// Remove it even when it has automation, a clip envelope, or a mapping
    /// (the warning accepted).
    #[serde(default)]
    pub force: bool,
}

/// Arguments of `plugin.move_configured_param`: moves one entry of the
/// device's configured list to another place in it (WP-5.40, manual 24.1.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveConfiguredParamArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// Where the parameter is now, 0 for the first.
    pub from: usize,
    /// Where it goes, 0 for the first.
    pub to: usize,
}

/// What `plugin.configure_param`, `plugin.unconfigure_param` and
/// `plugin.move_configured_param` return: the device's parameters, as
/// `device.parameters` reports them, each marked configured or not.
pub type ConfiguredParamsResult = crate::devices::DeviceParameters;

/// Arguments of `plugin.scan` and `plugin.rescan`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScanArgs {
    /// The files or folders to scan; each format's system plug-in folders
    /// if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    /// Scan the system folders in the background and return at once, with
    /// an empty report, as the Settings window's Rescan button does; the
    /// browser lists what was found when it's done.
    #[serde(default)]
    pub background: bool,
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
    /// Whether it sends notes on from a note output (IMP-24): such a
    /// plug-in can sit in the MIDI-effect position of a MIDI track's chain.
    pub note_output: bool,
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
    /// The preset names its devices hold, each once, in the order the
    /// devices come (the plug-in list spec, R5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preset_names: Vec<String>,
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

/// Arguments of `plugin.export_list`: where to write the list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportListArgs {
    /// The file to write the list to. Left out, the app asks where to
    /// save it, as the list's Export… button does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// What `plugin.export_list` returns: the file it wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportedList {
    /// The file the list was written to.
    pub path: String,
}

/// What the list of a set's plug-ins shows: whether it shows, and whether
/// Show Only Missing is on (the plug-in list spec, R10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PluginListState {
    /// Whether the list shows in the detail area's place.
    pub visible: bool,
    /// Whether Show Only Missing is on, so that only the plug-ins this
    /// computer doesn't have show.
    pub only_missing: bool,
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
    /// Use Audio Units v2 (Settings ▸ Plug-Ins): scan and list Audio Units
    /// on macOS. Turning it on scans them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_audio_units_v2: Option<bool>,
    /// Use Audio Units v3 (Settings ▸ Plug-Ins). Kept; app-extension Audio
    /// Units aren't scanned yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_audio_units_v3: Option<bool>,
    /// Use VST3 Plug-In System Folders (Settings ▸ Plug-Ins): scan and list
    /// the VST3 plug-ins in the format's system folders. Turning it on scans
    /// them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vst3_system_folders: Option<bool>,
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
    /// Use Audio Units v2.
    pub use_audio_units_v2: bool,
    /// Use Audio Units v3.
    pub use_audio_units_v3: bool,
    /// Use VST3 Plug-In System Folders.
    pub vst3_system_folders: bool,
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
                "Loads a plug-in from its file or bundle (`path`: a `.clap` file, a \
                 `.vst3` bundle or module, or on Linux an `.lv2` bundle folder) and adds it \
                 as a device, as `device.add` adds one: \
                 to the selected track (or `track`), at `index` or where its role goes, \
                 selected, one undo step (Insert Device). `plugin` picks one of the file's \
                 plug-ins by its ID (CLAP's, a VST3 class ID, or an LV2 URI); the first if \
                 left out. Its \
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
                 that reason. Its own editor window opens from `plugin.show_window`, as a \
                 CLAP plug-in's does, and a change made in it is one undo step. The unit's \
                 code runs inside Beatlayer, so load only units you trust.",
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
        CommandSpec::new(SHOW_MPE_SETTINGS, "MPE Settings…")
            .description(
                "Opens or closes the MPE/Multi-channel Settings dialog, from a plug-in's title \
                 bar menu, or from MIDI To once MPE is its channel: give a plug-in or External \
                 Instrument `device`, or a MIDI `track` whose MIDI To is a port set to MPE. \
                 Opens unless `shown` says which. The dialog's controls run \
                 `plugin.set_mpe_settings`, `routing.set_midi_output` and \
                 `device.set_external`. It changes only the view: it isn't an undo step. \
                 Returns the dialog's state.",
            )
            .permission(Permission::Read)
            .contexts([Global, DeviceView])
            .args::<ShowMpeSettingsArgs>()
            .result::<MpeSettingsDialog>(),
    )?;
    registry.register(
        CommandSpec::new(ARA_STATE, "ARA Plug-In State")
            .description(
                "Reports what each running ARA plug-in's document holds and has done: the \
                 devices bound to it and the tracks they render, how many region sequences, \
                 audio sources and audio modifications it has, each playback region with its \
                 place in the file and on the timeline in seconds, how many edit cycles the \
                 set has been given in, how many audio readers it opened and how much it read \
                 (never from the audio thread), the files it couldn't be given, and whether \
                 it restored its archive. A track with an ARA plug-in leaves its clips to \
                 it; the plug-in follows every clip edit, undo and redo as document edits. \
                 Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<AraStateArgs>()
            .result::<AraState>(),
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
        CommandSpec::new(CONFIGURE, "Configure Plug-In")
            .description(
                "Opens or closes a plug-in device's Plug-In Configure Mode (manual 24.1.2): \
                 while it is on, a parameter moved in the plug-in's own window joins the \
                 device's list of configured parameters, the ones its panel knobs and the \
                 automation and clip-envelope choosers show. `on` sets it, or switches it if \
                 left out. Configure mode itself is view state: it isn't an undo step and \
                 isn't saved with the set (the list is). Returns the Device View's state.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<ConfigureArgs>()
            .result::<ConfigureResult>(),
    )?;
    registry.register(
        CommandSpec::new(CONFIGURE_PARAM, "Configure Plug-In Parameter")
            .description(
                "Adds one of a plug-in's parameters to the device's list of configured \
                 parameters, the ones its panel knobs and the automation and clip-envelope \
                 choosers show (manual 24.1.2). `index` places it in the list, or the end if \
                 left out; one already in the list moves there. One undo step, and the list is \
                 saved with the set. Returns `device.parameters`, each marked configured or \
                 not.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<ConfigureParamArgs>()
            .result::<ConfiguredParamsResult>(),
    )?;
    registry.register(
        CommandSpec::new(UNCONFIGURE_PARAM, "Unconfigure Plug-In Parameter")
            .description(
                "Removes one parameter from a plug-in device's configured list (manual \
                 24.1.2). A parameter that has automation, a clip envelope, or a MIDI, key or \
                 Macro mapping is refused, naming what it has, as Live warns before deleting \
                 one; `force` removes it anyway. One undo step. Returns `device.parameters`, \
                 each marked configured or not.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<UnconfigureParamArgs>()
            .result::<ConfiguredParamsResult>(),
    )?;
    registry.register(
        CommandSpec::new(MOVE_CONFIGURED_PARAM, "Move Plug-In Parameter")
            .description(
                "Moves one entry of a plug-in device's configured list from `from` to `to` \
                 (manual 24.1.2), the order its panel knobs and the choosers show. One undo \
                 step. Returns `device.parameters`, each marked configured or not.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<MoveConfiguredParamArgs>()
            .result::<ConfiguredParamsResult>(),
    )?;
    registry.register(
        CommandSpec::new(SCAN, "Scan Plug-Ins")
            .description(
                "Finds the plug-ins installed on this machine and describes each one: its \
                 format, its file, its identifier, name, vendor, version, and role in a chain. \
                 Scans the files or folders in `paths` for every hosted format, or, when \
                 `paths` is left out, the system plug-in folders of CLAP, of LV2 on Linux \
                 (`LV2_PATH` names them when it is set), and of the formats \
                 the Plug-Ins settings switch on (`plugin.settings`: `vst3_system_folders`, \
                 and on macOS `use_audio_units_v2`). The app also runs this at start, in the \
                 background, and when one of those switches turns on. Each \
                 one is loaded in a separate process with a timeout, so a plug-in that crashes \
                 or hangs is put on a blocklist with the reason instead of taking Beatlayer \
                 down; a file whose size and modification time are unchanged is answered from \
                 the cache. Reads plug-in files outside the project, so an agent asks first.",
            )
            .permission(Permission::External)
            .contexts([Global, DeviceView])
            .args::<ScanArgs>()
            .result::<PluginScan>(),
    )?;
    registry.register(
        CommandSpec::new(RESCAN, "Rescan Plug-Ins")
            .description(
                "Scans the plug-ins in `paths` (or each format's system plug-in folders) \
                 again, ignoring the cache and clearing the blocklist for them, as the Rescan \
                 button does. Returns `plugin.scan`'s report; with `background`, returns at \
                 once and the browser lists what was found when the scan is done.",
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
                 when the scanner knows them, the devices and tracks that use it, the preset \
                 names its devices hold, and whether this machine has it (missing when the \
                 scanner hasn't seen it, or says why it is blocklisted). Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<ListArgs>()
            .result::<SetPluginList>(),
    )?;
    registry.register(
        CommandSpec::new(EXPORT_LIST, "Export the Set's Plug-In List")
            .description(
                "Writes the set's plug-in list to a text file, as the list's Export… button \
                 does: the set's name and the date, a header line, and one line per plug-in \
                 with its name, format, vendor, version, preset names, tracks, and `Missing` \
                 where this computer doesn't have it, separated by tabs. `path` is the file to \
                 write it to; left out, the app asks where to save it. The file is written \
                 outside the set, so an agent asks first. Returns the file written.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<ExportListArgs>()
            .result::<ExportedList>(),
    )?;
    registry.register(
        CommandSpec::new(SETTINGS, "Plug-In Settings")
            .description(
                "Reads or changes how the browser shows plug-ins (IMP-07): `hidden` is the \
                 plug-ins to hide, as `format/plugin_id` keys; `show_hidden` whether hidden \
                 plug-ins still show; and `format_preference` the format to prefer when a \
                 plug-in exists in several. Also the Plug-In Sources switches of Settings ▸ \
                 Plug-Ins: `use_audio_units_v2`, `use_audio_units_v3` and \
                 `vst3_system_folders`, all off in a new install; the browser lists a format's \
                 plug-ins only while its switch is on (CLAP has none and is always listed), \
                 and turning one on scans that format in the background. Fields left out are \
                 unchanged. Returns the settings. A set that uses a hidden plug-in still loads \
                 it.",
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
