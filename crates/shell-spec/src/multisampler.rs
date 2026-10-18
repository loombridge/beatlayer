//! The multisampler's commands that run on the shell (WP-5.13; the
//! multisampler spec, R3, R7, R10, R11, R13, R20, R30): adding layers from
//! files, changing their settings with Snap, replacing a layer's sample,
//! normalising, and what the multisampler's views show.
//!
//! The document's own layer commands (`device.insert_layers`,
//! `device.delete_layers`, `device.duplicate_layers`, `device.rename_layer`,
//! `device.move_layers`, `device.sort_layers`, `device.set_layer_zone`,
//! `device.update_layers`, `device.set_layer_media`,
//! `device.distribute_layers` and `device.set_layer_options`) are
//! `bl-model`'s. Those here read files and audio or know the selection. A
//! command whose `device` is left out acts on the device selected in the
//! Device View.

use bl_commands::Context::DeviceView;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::{
    DeviceId, EditorSize, LayerId, LayerSettings, MultisamplerView, SamplerTab, ZoneKind,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `device.add_layers`: add layers from files (R3).
pub const ADD_LAYERS: &str = "device.add_layers";
/// `device.set_layer`: change layers' settings, snapping markers while
/// Snap is on (R20).
pub const SET_LAYER: &str = "device.set_layer";
/// `device.snap_layer_markers`: snap a loop's markers once (R20).
pub const SNAP_LAYER_MARKERS: &str = "device.snap_layer_markers";
/// `device.set_layer_sample`: replace a layer's sample with a file (R10).
pub const SET_LAYER_SAMPLE: &str = "device.set_layer_sample";
/// `device.normalize_layers`: Normalize Volume and Normalize Pan (R13).
pub const NORMALIZE_LAYERS: &str = "device.normalize_layers";
/// `device.set_multisampler_view`: the tab, the zone editor, Auto Select.
pub const SET_VIEW: &str = "device.set_multisampler_view";
/// `device.select_layers`: select layers in the layer list (R11).
pub const SELECT_LAYERS: &str = "device.select_layers";

/// `device.hot_swap_layer`: start or end hot-swapping a layer's sample
/// (R10).
pub const HOT_SWAP_LAYER: &str = "device.hot_swap_layer";

/// The undo step's name for adding layers from files: Live's name for
/// dropping samples on its layer list is to observe.
pub const ADD_LAYERS_STEP: &str = "Add Samples";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[
    ADD_LAYERS,
    SET_LAYER,
    SNAP_LAYER_MARKERS,
    SET_LAYER_SAMPLE,
    NORMALIZE_LAYERS,
    SET_VIEW,
    SELECT_LAYERS,
    HOT_SWAP_LAYER,
];

/// Whether `id` is one of these commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `device.add_layers`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddLayersArgs {
    /// The multisampler; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The audio files, one layer each.
    pub paths: Vec<String>,
    /// Where in the layer list the first goes; after the selected layers
    /// (or at the end with none selected) if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// What `device.add_layers` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LayersAdded {
    /// The multisampler.
    pub device: DeviceId,
    /// The new layers, in list order: now the selected layers.
    pub layers: Vec<LayerId>,
    /// The files that weren't added, each with why.
    pub skipped: Vec<String>,
}

/// Arguments of `device.set_layer`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLayerArgs {
    /// The multisampler; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The layers; the selected layers if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layers: Option<Vec<LayerId>>,
    /// The settings to set; the ones left out stay as they were.
    pub settings: LayerSettings,
}

/// What the layer commands return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LayersChanged {
    /// The multisampler.
    pub device: DeviceId,
    /// The layers the command changed.
    pub layers: Vec<LayerId>,
}

/// Which of a layer's loops `device.snap_layer_markers` snaps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopKind {
    /// The sustain loop's Loop Start and Loop End.
    #[default]
    Sustain,
    /// The release loop's point.
    Release,
}

/// Arguments of `device.snap_layer_markers`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapMarkersArgs {
    /// The multisampler; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The layers; the selected layers if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layers: Option<Vec<LayerId>>,
    /// Which loop's markers; the sustain loop's if left out.
    #[serde(default, rename = "loop")]
    pub which: LoopKind,
}

/// Arguments of `device.set_layer_sample`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLayerSampleArgs {
    /// The multisampler; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The layer; the first selected layer if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<LayerId>,
    /// The audio file.
    pub path: String,
}

/// What `device.normalize_layers` sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Normalize {
    /// Each layer's Volume, so its peak reaches 0 dBFS.
    Volume,
    /// Each layer's Pan, so its channels play at equal level.
    Pan,
}

/// Arguments of `device.normalize_layers`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NormalizeArgs {
    /// The multisampler; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The layers; the selected layers if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layers: Option<Vec<LayerId>>,
    /// What to normalise.
    pub what: Normalize,
}

/// Arguments of `device.set_multisampler_view`: each left out stays as it
/// was.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetViewArgs {
    /// The multisampler; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The tab shown in the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<SamplerTab>,
    /// Whether the zone editor shows above the Device View (the Zone tab).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_editor: Option<bool>,
    /// Which zone editor shows: Key, Vel or Sel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<ZoneKind>,
    /// The zone editor's row size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<EditorSize>,
    /// Auto Select: notes select the layers they play.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_select: Option<bool>,
}

/// How `device.select_layers` changes the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LayerSelectMode {
    /// The listed layers become the selection (a click).
    #[default]
    Replace,
    /// They join it (Shift-click).
    Add,
    /// Each leaves the selection if it's in it and joins it if not
    /// (Cmd-click).
    Toggle,
    /// The selection becomes every layer whose range in the editor in view
    /// equals the first listed layer's (Select All With Same Range).
    SameRange,
}

/// Arguments of `device.select_layers`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectLayersArgs {
    /// The multisampler; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The layers; every layer if left out with `replace`.
    #[serde(default)]
    pub layers: Vec<LayerId>,
    /// How they change the selection.
    #[serde(default)]
    pub mode: LayerSelectMode,
}

/// Arguments of `device.hot_swap_layer`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HotSwapLayerArgs {
    /// The multisampler; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The layer; the first selected layer if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<LayerId>,
    /// Start (true) or end (false) hot-swapping; starts if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// A multisampler's views and selection, as `device_view.state` reports
/// them (R30).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MultisamplerState {
    /// What its views show.
    #[serde(flatten)]
    pub view: MultisamplerView,
    /// The selected layers, in list order.
    pub selection: Vec<LayerId>,
    /// How many layers it holds.
    pub layers: usize,
    /// The layer whose sample is being hot-swapped, if one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hot_swap: Option<LayerId>,
}

/// Adds the multisampler's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(ADD_LAYERS, "Add Samples")
            .description(
                "Adds audio files to a multisampler as layers, as dropping them on the zone \
                 editor's layer list does: each file is imported into the set's media, and \
                 gets one layer after the selected layers (or at the end), or at `index`, with \
                 every zone whole and unfaded, its Root Key from the file's sampler metadata \
                 (C3 where it has none), Sample Start and End at the file's ends, and its loop \
                 points from the file's first loop with Sustain Mode left off. The new layers \
                 become the selected ones. A file that isn't audio is skipped and named in \
                 `skipped`. One undo step. It reads files outside the set.",
            )
            .permission(Permission::External)
            .context(DeviceView)
            .args::<AddLayersArgs>()
            .result::<LayersAdded>(),
    )?;
    registry.register(
        CommandSpec::new(SET_LAYER, "Change Sample Settings")
            .description(
                "Changes settings of layers (`layers`, or the selected ones) as the Sample tab \
                 does: Root Key, Detune, Volume (a linear gain), Pan, Sample Start and End, \
                 Link, the sustain and release modes, Loop Start and End, the Release Loop \
                 point, the loop's Crossfade and Detune. While the multisampler's Snap is on, \
                 every marker set lands on the nearest zero crossing of the layer's left \
                 channel. Markers are in frames of the sample at its own rate. The markers \
                 must stay ordered; one setting out of range refuses the whole change. One \
                 undo step. `device.update_layers` sets them exactly, without snapping.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<SetLayerArgs>()
            .result::<LayersChanged>(),
    )?;
    registry.register(
        CommandSpec::new(SNAP_LAYER_MARKERS, "Snap Markers")
            .description(
                "Snaps a loop's two markers (the sustain loop's Loop Start and Loop End, or the \
                 release loop's point) to the nearest zero crossings of the layers' left \
                 channel once, whatever Snap says, as the loop brace's context menu does. One \
                 undo step.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<SnapMarkersArgs>()
            .result::<LayersChanged>(),
    )?;
    registry.register(
        CommandSpec::new(SET_LAYER_SAMPLE, "Replace Sample")
            .description(
                "Replaces a layer's sample (`layer`, or the first selected layer) with an audio \
                 file, keeping its zones and settings and its markers at the same share of the \
                 new length, as hot-swapping a sample does. The file is imported into the \
                 set's media. One undo step. It reads a file outside the set.",
            )
            .permission(Permission::External)
            .context(DeviceView)
            .args::<SetLayerSampleArgs>()
            .result::<LayersChanged>(),
    )?;
    registry.register(
        CommandSpec::new(NORMALIZE_LAYERS, "Normalize")
            .description(
                "Normalize Volume sets each layer's Volume (`layers`, or the selected ones) so \
                 its sample's largest magnitude between Sample Start and Sample End reaches \
                 0 dBFS; Normalize Pan sets each layer's Pan so its left and right channels \
                 play at equal RMS level after the pan. A silent layer, and a mono one for \
                 Normalize Pan, keep their value. One undo step.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<NormalizeArgs>()
            .result::<LayersChanged>(),
    )?;
    registry.register(
        CommandSpec::new(SET_VIEW, "Set Multisampler View")
            .description(
                "Sets what a multisampler's views show, as its title bar's tabs and the zone \
                 editor's controls do: the `tab` in its body (`sample`, `pitch_osc`, \
                 `filter_global`, `modulation` or `midi`), whether the zone editor shows above \
                 the Device View (`zone_editor`, the Zone tab), which editor shows (`editor`: \
                 `key`, `velocity` or `select`), its row `size`, and Auto Select. The views \
                 are saved with the set but are no undo step, and change nothing in the \
                 sound. They read back in `device_view.state`.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<SetViewArgs>()
            .result::<MultisamplerState>(),
    )?;
    registry.register(
        CommandSpec::new(SELECT_LAYERS, "Select Samples")
            .description(
                "Selects layers in a multisampler's layer list, as clicking does: `replace` \
                 makes the listed layers the selection (every layer with none listed), `add` \
                 joins them to it, `toggle` flips each, and `same_range` selects every layer \
                 whose range in the editor in view equals the first listed layer's. The \
                 selection shows in the Sample tab and reads back in `device_view.state`. It \
                 changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<SelectLayersArgs>()
            .result::<MultisamplerState>(),
    )?;
    registry.register(
        CommandSpec::new(HOT_SWAP_LAYER, "Hot-Swap Sample")
            .description(
                "Starts hot-swapping a layer's sample (`layer`, or the first selected layer), \
                 as the zone editor's Hot-Swap Sample button does: the sample the browser then \
                 loads (Enter or a double-click on a file) replaces the layer's, keeping its \
                 zones and settings, as one undo step. `on: false` ends it. It changes \
                 nothing in the set by itself.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<HotSwapLayerArgs>()
            .result::<MultisamplerState>(),
    )?;
    Ok(())
}
