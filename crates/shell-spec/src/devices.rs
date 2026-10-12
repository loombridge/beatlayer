//! The Device View's commands in the command registry (WP-2.4): what the
//! Device View shows, selecting and folding devices, adding them as a
//! person does, the clipboard, presets, and hot-swapping.
//!
//! The document's own device commands (`device.insert`, `device.remove`,
//! `device.move`, `device.rename`, `device.set_enabled`,
//! `device.set_show_preset_name`, `device.set_state`, `device.set_param`)
//! are `bl-model`'s. Those here run on the shell, which knows the selection,
//! the clipboard, the User Library, and the browser. A command whose
//! `device` is left out acts on the device selected in the Device View, as
//! its key and its context-menu item do. The behaviour they follow is in the
//! device chains spec.

use bl_commands::Context::{Browser, DeviceView, Global};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{DeviceId, DeviceKind, DeviceRole, TrackId, TrackKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `device_view.state`: what the Device View shows.
pub const STATE: &str = "device_view.state";
/// `device.add`: add a device as a person does.
pub const ADD: &str = "device.add";
/// `device.select`: select a device.
pub const SELECT: &str = "device.select";
/// `device.fold`: fold or unfold a device.
pub const FOLD: &str = "device.fold";
/// `device.cut`: cut a device to the clipboard.
pub const CUT: &str = "device.cut";
/// `device.copy`: copy a device to the clipboard.
pub const COPY: &str = "device.copy";
/// `device.paste`: paste the clipboard's device.
pub const PASTE: &str = "device.paste";
/// `device.duplicate`: duplicate a device.
pub const DUPLICATE: &str = "device.duplicate";
/// `device.save_preset`: save a device's state as a preset.
pub const SAVE_PRESET: &str = "device.save_preset";
/// `device.save_default_preset`: save a device's state as its type's
/// default.
pub const SAVE_DEFAULT_PRESET: &str = "device.save_default_preset";
/// `device.load_preset`: load a preset into a device.
pub const LOAD_PRESET: &str = "device.load_preset";
/// `device.hot_swap`: start or end hot-swapping a device's presets.
pub const HOT_SWAP: &str = "device.hot_swap";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    STATE,
    ADD,
    SELECT,
    FOLD,
    CUT,
    COPY,
    PASTE,
    DUPLICATE,
    SAVE_PRESET,
    SAVE_DEFAULT_PRESET,
    LOAD_PRESET,
    HOT_SWAP,
];

/// The document's device commands that act on the selected device when
/// `device` is left out, as their keys and menu items do.
pub const ON_SELECTED: &[&str] = &[
    "device.remove",
    "device.rename",
    "device.set_enabled",
    "device.set_show_preset_name",
];

/// Whether `id` is one of the Device View's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// A device frame's title bar height at 100%, in points (observed in Live
/// 12.4.6; layout L-DEV-004): also a folded device's width.
pub const TITLE_BAR_HEIGHT: f32 = 16.0;

/// The gap between devices in a chain at 100%, in points (observed).
pub const DEVICE_GAP: f32 = 7.0;

/// An unfolded device's width at 100%, in points, until devices give their
/// own (WP-2.5): Live's are each device's own.
pub const DEFAULT_DEVICE_WIDTH: f32 = 120.0;

/// One device as the Device View shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceFrame {
    /// The device.
    pub id: DeviceId,
    /// What its title bar reads: its preset's or given name while Show
    /// Preset Name is on, otherwise its own name.
    pub title: String,
    /// Its own name, such as `Auto Filter`.
    pub name: String,
    /// What it is.
    pub kind: DeviceKind,
    /// Where it goes in a chain.
    pub role: DeviceRole,
    /// Whether it's switched on.
    pub enabled: bool,
    /// Whether its title shows its preset's or given name.
    pub show_preset_name: bool,
    /// Whether it's folded to its title bar.
    pub folded: bool,
    /// Whether it's the selected device.
    pub selected: bool,
    /// Whether it's the device being hot-swapped.
    pub hot_swap: bool,
}

/// What `device_view.state` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceViewState {
    /// The track whose chain shows: the selected track, or `null` with no
    /// track selected.
    pub track: Option<TrackId>,
    /// That track's kind.
    pub track_kind: Option<TrackKind>,
    /// Its devices, in signal order.
    pub devices: Vec<DeviceFrame>,
    /// The selected device, if it's in this chain.
    pub selected: Option<DeviceId>,
    /// The device being hot-swapped, on any track, and what the browser's
    /// bar says ("Swapping Audio Effect").
    pub hot_swap: Option<HotSwap>,
    /// Whether the clipboard holds a device to paste.
    pub can_paste: bool,
}

/// Hot-swapping, while it lasts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HotSwap {
    /// The device a preset loads into.
    pub device: DeviceId,
    /// The browser's bar: `Swapping Audio Effect`, `Swapping Instrument`, or
    /// `Swapping MIDI Effect`.
    pub label: String,
}

/// Arguments of the commands that act on one device: the selected device
/// if `device` is left out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
}

/// Arguments of `device.select`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectDeviceArgs {
    /// The device, or `null` to select none.
    pub device: Option<DeviceId>,
}

/// Arguments of `device.fold`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FoldDeviceArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// Fold (true) or unfold (false); switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folded: Option<bool>,
}

/// The device `device.add` adds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewDevice {
    /// What it is: a native device's type ID or a plug-in's format and ID.
    pub kind: DeviceKind,
    /// Its own name, such as the browser lists it.
    pub name: String,
    /// Where it goes in a chain.
    #[serde(default)]
    pub role: DeviceRole,
}

/// Arguments of `device.add`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddDeviceArgs {
    /// The device to add.
    pub device: NewDevice,
    /// The track; the selected track if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// Where it goes in the chain; after the selected device, or at the end
    /// of its role's place in the chain, if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// What `device.add`, `device.paste`, and `device.duplicate` return.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceAdded {
    /// The new device, now selected.
    pub device: DeviceId,
    /// Its track.
    pub track: TrackId,
    /// Whether a new MIDI track was made for it: an instrument or MIDI
    /// effect added while an audio, group, or return track was selected.
    pub new_track: bool,
    /// The default preset it loaded, if its type has one.
    pub default_preset: Option<String>,
}

/// Arguments of `device.paste`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasteDeviceArgs {
    /// The track; the selected track if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// Paste before this device; before the selected device, or at the end
    /// of the chain with none selected, if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<DeviceId>,
}

/// Arguments of `device.save_preset`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavePresetArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The file to write, ending `.blpreset`; a new file in the User
    /// Library's folder for the device's presets, named after its title, if
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Arguments of `device.load_preset`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadPresetArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The preset file.
    pub path: String,
}

/// What the preset commands return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresetResult {
    /// The device.
    pub device: DeviceId,
    /// The preset file written or read.
    pub path: String,
    /// The preset's name: its file's name without the extension.
    pub name: String,
}

/// Arguments of `device.hot_swap`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HotSwapArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// Start (true) or end (false) hot-swapping; switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Adds the Device View's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs() {
        registry.register(spec)?;
    }
    Ok(())
}

#[expect(clippy::too_many_lines, reason = "one spec per command, in order")]
fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(STATE, "Device View State")
            .description(
                "Reports what the Device View shows: the selected track and its devices in \
                 signal order, each with its title, its own name, its kind and role, whether \
                 it's on, folded, selected, or being hot-swapped; the selected device; \
                 hot-swapping; and whether there's a device to paste. Device parameters are in \
                 the set; read them with the query tool. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(ADD, "Add Device")
            .description(
                "Adds a device as a person does from the browser: to the selected track (or \
                 `track`), after the selected device or at the end of its role's place in the \
                 chain (or at `index`), switched on, with its type's default preset if one was \
                 saved, and selects it. The chain keeps Live's order: MIDI effects, the \
                 instrument, then audio effects. An instrument or MIDI effect added while an \
                 audio, group, or return track is selected goes on a new MIDI track at the end \
                 of the track list, which becomes the selected track, as one undo step (Insert \
                 Device and Track). A chain holds one instrument until racks arrive.",
            )
            .permission(Permission::Edit)
            .contexts([DeviceView, Browser])
            .args::<AddDeviceArgs>()
            .result::<DeviceAdded>(),
        CommandSpec::new(SELECT, "Select Device")
            .description(
                "Selects a device, as clicking its title bar does, or none with `null`. The \
                 Device View's keys (Delete, Cmd-C, Cmd-D, Cmd-R) act on it. Selecting \
                 changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<SelectDeviceArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(FOLD, "Fold Device")
            .description(
                "Folds a device to a narrow strip showing only its title bar on its side, or \
                 unfolds it, as double-clicking its title bar or the context menu's Fold does; \
                 switches unless `folded` says which. It changes only the view: it isn't an undo \
                 step, and isn't saved with the set.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .binding("S-DEV-014")
            .args::<FoldDeviceArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(CUT, "Cut Device")
            .description(
                "Copies a device to the clipboard, with its settings, and deletes it from its \
                 chain: one undo step, named Delete Device(s). Because it removes the person's \
                 work an agent asks first. `device.paste` puts it back, anywhere.",
            )
            .permission(Permission::Destructive)
            .context(DeviceView)
            .binding("S-EDIT-001")
            .args::<DeviceArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(COPY, "Copy Device")
            .description(
                "Copies a device to the clipboard, with its settings, for `device.paste`. \
                 Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .binding("S-EDIT-002")
            .args::<DeviceArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(PASTE, "Paste Device")
            .description(
                "Pastes the clipboard's device, as a new device with its settings, before the \
                 selected device (or `before`), or at the end of the chain with none selected, \
                 on the selected track (or `track`), and selects it: one undo step, named Copy \
                 Device. The chain keeps Live's order, so an audio effect can't go before an \
                 instrument, and instruments and MIDI effects only go on MIDI tracks.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .binding("S-EDIT-003")
            .args::<PasteDeviceArgs>()
            .result::<DeviceAdded>(),
        CommandSpec::new(DUPLICATE, "Duplicate Device")
            .description(
                "Puts a copy of a device, with its settings, straight after it, and selects the \
                 copy: one undo step, named Duplicate Device. A chain holds one instrument until \
                 racks arrive, so an instrument can't be duplicated yet.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .binding("S-EDIT-004")
            .args::<DeviceArgs>()
            .result::<DeviceAdded>(),
        CommandSpec::new(SAVE_PRESET, "Save Preset")
            .description(
                "Saves a device's state (its parameter values) as a Beatlayer preset file \
                 (.blpreset), as the title bar's Save Preset button does: to `path`, or to a \
                 new file named after the device's title in the User Library, under \
                 Presets/<Audio Effects, Instruments, or MIDI Effects>/<the device's own \
                 name>/, numbered if the name is taken. The device's title then shows the \
                 preset's name. Writes a file outside the project.",
            )
            .permission(Permission::External)
            .context(DeviceView)
            .args::<SavePresetArgs>()
            .result::<PresetResult>(),
        CommandSpec::new(SAVE_DEFAULT_PRESET, "Save as Default Preset")
            .description(
                "Saves a device's state as the default preset for its type, as the context \
                 menu's Save as Default Preset does: to the User Library's Defaults/<Audio \
                 Effects, Instruments, or MIDI Effects>/<the device's own name>.blpreset, \
                 replacing any default saved before. Every device of that type added from then \
                 on (`device.add`) starts with it. Writes a file outside the project.",
            )
            .permission(Permission::External)
            .context(DeviceView)
            .args::<DeviceArgs>()
            .result::<PresetResult>(),
        CommandSpec::new(LOAD_PRESET, "Load Preset")
            .description(
                "Loads a Beatlayer preset file (.blpreset) into a device of the kind it was \
                 saved from, as choosing it while hot-swapping does: every parameter takes the \
                 preset's value (or its default, if the preset doesn't set it), and the \
                 device's title shows the preset's name. One undo step. A preset for another \
                 kind of device is refused. Reads a file outside the project.",
            )
            .permission(Permission::External)
            .context(DeviceView)
            .args::<LoadPresetArgs>()
            .result::<PresetResult>(),
        CommandSpec::new(HOT_SWAP, "Hot-Swap Presets")
            .description(
                "Starts or ends hot-swapping a device's presets, as the title bar's Hot-Swap \
                 Presets button does; switches unless `on` says which. While it lasts, the \
                 browser shows a bar (Swapping Audio Effect) and opens the User Library's \
                 folder of the device's presets, and loading a preset from the browser (Enter \
                 or a double-click) loads it into the device. Changes only the view.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<HotSwapArgs>()
            .result::<DeviceViewState>(),
    ]
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
