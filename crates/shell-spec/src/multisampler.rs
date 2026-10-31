//! The multisampler's commands that run on the shell (WP-5.13 to WP-5.15;
//! the multisampler spec, R3, R7, R10, R11, R13, R20, R30, R47): adding
//! layers from files, importing an SFZ file, changing their settings with
//! Snap, replacing a layer's sample, normalising, and what the
//! multisampler's views show; and the commands that edit the samples of both
//! sampler devices (WP-5.15, R55 to R63): the edit list, Crop and Reverse as
//! processed files, the conversions between Simpler and the multisampler,
//! and auditioning from a point.
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
    ContentHash, DeviceId, EditorSize, LayerId, LayerSettings, MultisamplerView, SampleEdit,
    SampleEditKind, SampleRange, SamplerTab, TrackId, ZoneKind,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `device.add_layers`: add layers from files (R3).
pub const ADD_LAYERS: &str = "device.add_layers";
/// `device.import_sfz`: import an SFZ file as a multisampler's layers (R47).
pub const IMPORT_SFZ: &str = "device.import_sfz";
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

/// `device.edit_sample`: add an edit to a sample's list, or revert them
/// (R55, R58).
pub const EDIT_SAMPLE: &str = "device.edit_sample";
/// `device.crop_sample`: write the region as a processed copy (R59).
pub const CROP_SAMPLE: &str = "device.crop_sample";
/// `device.reverse_sample`: write the sample reversed as a processed copy
/// (R59).
pub const REVERSE_SAMPLE: &str = "device.reverse_sample";
/// `device.convert`: convert Simpler and the multisampler into each other
/// (R62).
pub const CONVERT: &str = "device.convert";
/// `device.audition_sample`: play a sample from a point through its device
/// (R54).
pub const AUDITION_SAMPLE: &str = "device.audition_sample";
/// `device.set_sample_region`: move Simpler's Start and End flags (the
/// sampler spec, R22).
pub const SET_SAMPLE_REGION: &str = "device.set_sample_region";

/// The undo step's name for adding layers from files: Live's name for
/// dropping samples on its layer list is to observe.
pub const ADD_LAYERS_STEP: &str = "Add Samples";

/// The undo step's name for importing an SFZ file (to observe: Live has no
/// such import).
pub const IMPORT_SFZ_STEP: &str = "Import SFZ";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[
    ADD_LAYERS,
    IMPORT_SFZ,
    SET_LAYER,
    SNAP_LAYER_MARKERS,
    SET_LAYER_SAMPLE,
    NORMALIZE_LAYERS,
    SET_VIEW,
    SELECT_LAYERS,
    HOT_SWAP_LAYER,
    EDIT_SAMPLE,
    CROP_SAMPLE,
    REVERSE_SAMPLE,
    CONVERT,
    AUDITION_SAMPLE,
    SET_SAMPLE_REGION,
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
    /// Puts the first straight after this layer, instead of at `index` or
    /// after the selected ones (R61).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<LayerId>,
    /// Gives each new layer this layer's key, velocity and sample select
    /// zones with their fades, so the two sound together (R61).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub like: Option<LayerId>,
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

/// Arguments of `device.import_sfz`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportSfzArgs {
    /// The `.sfz` file.
    pub path: String,
    /// A multisampler to add the regions to as layers, at the end of its
    /// list. Left out, the import makes a multisampler: on `track`, or on a
    /// new MIDI track below the last track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The MIDI track to put a new multisampler on, after its other devices.
    /// Ignored with `device`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
}

/// An opcode or header the import left unread or settled device-wide, with
/// how many regions use it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SfzCount {
    /// The opcode, or the header in angle brackets.
    pub name: String,
    /// How many regions use it.
    pub regions: usize,
}

/// A region the import left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SfzLeftOut {
    /// Its place among the file's regions, from 1.
    pub region: usize,
    /// Its sample as the file names it.
    pub sample: String,
    /// Why.
    pub reason: String,
}

/// What `device.import_sfz` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SfzImported {
    /// The multisampler.
    pub device: DeviceId,
    /// Its track.
    pub track: TrackId,
    /// The new layers, in list order: now the selected layers.
    pub layers: Vec<LayerId>,
    /// Opcodes and headers left unread.
    pub unread: Vec<SfzCount>,
    /// Opcodes most regions agree on, which set the device's own controls.
    pub settled: Vec<SfzCount>,
    /// Regions left out, each with why.
    pub left_out: Vec<SfzLeftOut>,
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

/// Arguments of `device.edit_sample`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditSampleArgs {
    /// The sampler device; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// A multisampler's layer; its first selected layer if left out.
    /// Simpler has one sample and no layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<LayerId>,
    /// What the edit does. Leave it out with `revert`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SampleEditKind>,
    /// The selection it acts on, in frames of the sample as the earlier
    /// edits left it, at its own rate. Left out it acts on the whole
    /// sample; Trim and Cut need one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<SampleRange>,
    /// Clears the sample's edit list instead of adding to it.
    #[serde(default)]
    pub revert: bool,
}

/// What `device.edit_sample` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SampleEdited {
    /// The sampler device.
    pub device: DeviceId,
    /// The layer edited; none for Simpler's sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<LayerId>,
    /// The sample's edit list now.
    pub edits: Vec<SampleEdit>,
    /// The edited sample's length in frames at its own rate.
    pub length: u64,
}

/// Arguments of `device.crop_sample` and `device.reverse_sample`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessSampleArgs {
    /// The sampler device; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// A multisampler's layer; its first selected layer if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<LayerId>,
}

/// What `device.crop_sample` and `device.reverse_sample` return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SampleProcessed {
    /// The sampler device.
    pub device: DeviceId,
    /// The layer; none for Simpler's sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<LayerId>,
    /// The processed copy, now the sample's media.
    pub media: ContentHash,
    /// Where the copy is in the project folder.
    pub path: String,
    /// Its length in frames at its own rate.
    pub length: u64,
}

/// What `device.convert` makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConvertTo {
    /// Simpler, from a multisampler.
    Snapshot,
    /// A multisampler, from Simpler.
    Multisampler,
}

/// Arguments of `device.convert`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConvertArgs {
    /// The sampler device; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// What to convert it to.
    pub to: ConvertTo,
}

/// What `device.convert` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Converted {
    /// The new device, in the old one's place.
    pub device: DeviceId,
    /// What it is.
    pub to: ConvertTo,
    /// What of the old device didn't carry over, each in a sentence.
    pub dropped: Vec<String>,
}

/// Arguments of `device.audition_sample`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditionArgs {
    /// The sampler device; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// A multisampler's layer; its first selected layer if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<LayerId>,
    /// Where in the sample to start, as a share of its length from 0 to 1.
    #[serde(default)]
    pub from: f64,
    /// Whether the button goes down (`true`, the default) or comes up.
    #[serde(default = "yes")]
    pub down: bool,
}

fn yes() -> bool {
    true
}

/// Arguments of `device.set_sample_region`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSampleRegionArgs {
    /// Simpler; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The Start flag in frames of the edited sample at its own rate; it
    /// stays where it is if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    /// The End flag, in the same frames; it stays where it is if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
}

/// What `device.set_sample_region` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SampleRegionSet {
    /// Simpler.
    pub device: DeviceId,
    /// The Start flag now.
    pub start: u64,
    /// The End flag now.
    pub end: u64,
    /// The edited sample's length in frames at its own rate.
    pub length: u64,
}

/// What `device.audition_sample` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Auditioned {
    /// Whether the audio engine took it: it is off, or has no device, when
    /// this is `false`, and nothing plays.
    pub sent: bool,
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
                 gets one layer after the selected layers (or at the end), or at `index`, or \
                 straight after the layer `after`, with every zone whole and unfaded (or \
                 copied from the layer `like`, so the new layers sound together with it), its Root Key from the file's sampler metadata \
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
        CommandSpec::new(IMPORT_SFZ, "Import SFZ")
            .description(
                "Imports an SFZ multisample instrument, the open text format, as a \
                 multisampler: each region becomes a layer (its sample, key and velocity \
                 zones with their crossfades as fades, Root Key, tuning, Volume, Pan, start \
                 and end points, loop and loop mode), and the device-wide settings most \
                 regions agree on (the volume, filter, filter and pitch envelopes, Vel > Vol, \
                 Pitch Bend Range, reverse, the fade law) set the new device's controls. \
                 Sequences (`seq_length`) turn Round Robin on in Forward mode, random windows \
                 (`lorand`) in Random mode. With `device`, a multisampler, only the layers are \
                 added, at the end of its list; otherwise a multisampler is made on `track`, \
                 or on a new MIDI track below the last. The samples join the set's media where \
                 they lie. Regions whose sample is missing or isn't audio, or whose `trigger` \
                 isn't `attack`, are left out and named in `left_out`; opcodes the import \
                 doesn't read are listed in `unread`. A file with no region left changes \
                 nothing. One undo step. It reads files outside the set.",
            )
            .permission(Permission::External)
            .context(DeviceView)
            .args::<ImportSfzArgs>()
            .result::<SfzImported>(),
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
    register_sample_edits(registry)
}

/// The commands that edit the samples of Simpler and the multisampler
/// (WP-5.15).
fn register_sample_edits(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(EDIT_SAMPLE, "Edit Sample")
            .description(
                "Edits a sampler's sample without writing a file, as the sample display's \
                 context menu does: `kind` is `trim` (keep the selection), `cut` (remove it \
                 and close the gap), `reverse`, `fade_in`, `fade_out` (a straight line in gain \
                 over the selection) or `normalize` (scale the selection so its largest \
                 magnitude reaches 0 dBFS). `range` is the selection, `from` to `to` in frames \
                 of the sample as the earlier edits left it, at its own rate; left out it is \
                 the whole sample, except for trim and cut, which need one. The edit joins the \
                 sample's list, kept with the set and applied to the file's audio in order; the \
                 file is never written. A multisampler's `layer` (its first selected layer if \
                 left out) keeps its markers on the audio they marked: those after a cut move \
                 earlier by its length, those inside it to its start, and those outside a trim \
                 to its nearer end; Simpler's Start, Length and Loop are shares of the sample \
                 and stay. `revert: true` clears the list instead. One undo step named for the \
                 edit. The list reads back as `sample_edits` in the query tool's `device` \
                 result for Simpler and as `edits` in each layer of `layers` for the \
                 multisampler.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<EditSampleArgs>()
            .result::<SampleEdited>(),
    )?;
    registry.register(
        CommandSpec::new(CROP_SAMPLE, "Crop Sample")
            .description(
                "Removes the sample outside Simpler's region, or outside a multisampler \
                 layer's Sample Start and Sample End, by writing a processed copy: a WAV with \
                 the sample's edits applied, at its rate and channels, in the project's \
                 `Samples/Processed/Crop/` folder. The device or layer then plays the copy, \
                 with its edit list empty and the region the whole of it, as one undo step; \
                 the original file stays as it was. A set that has never been saved has no \
                 project folder, and the command is refused with a message in the Status Bar. \
                 The copy is the sample's new media, which reads back in the device's `media` \
                 or the layer's `media`.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<ProcessSampleArgs>()
            .result::<SampleProcessed>(),
    )?;
    registry.register(
        CommandSpec::new(REVERSE_SAMPLE, "Reverse Sample")
            .description(
                "Reverses the whole sample by writing a processed copy: a WAV with the sample's \
                 edits applied and then reversed, at its rate and channels, in the project's \
                 `Samples/Processed/Reverse/` folder. The device or layer then plays the copy \
                 with its edit list empty and its markers where they were, as one undo step; \
                 the original file stays as it was. A set that has never been saved is refused \
                 with a message in the Status Bar. The copy reads back as the device's or \
                 layer's `media`. `device.edit_sample` with `kind: reverse` reverses without a \
                 file.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<ProcessSampleArgs>()
            .result::<SampleProcessed>(),
    )?;
    registry.register(
        CommandSpec::new(CONVERT, "Convert Sampler")
            .description(
                "Converts Simpler to a multisampler (`to: multisampler`) or a multisampler to \
                 Simpler (`to: snapshot`), keeping the device's place in its chain, its name, \
                 its on or off state and its sample, as one undo step. Simpler becomes one \
                 layer with every zone whole, its region as Sample Start and Sample End, its \
                 loop as a sustain loop and its edits, with its envelope, volume and pan as the \
                 multisampler's; a multisampler becomes Simpler in Classic mode playing its \
                 first layer. Slicing and One-Shot, and layers past the first, don't carry \
                 over, and the result's `dropped` names what didn't. The new device has a new \
                 ID, which the result gives.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<ConvertArgs>()
            .result::<Converted>(),
    )?;
    registry.register(
        CommandSpec::new(AUDITION_SAMPLE, "Audition Sample")
            .description(
                "Plays a sampler's sample from a point through the device, as a click in its \
                 display does: `from` is the start as a share of the sample from 0 to 1, \
                 `layer` a multisampler's layer (its first selected layer if left out). The \
                 voice plays at the sample's root pitch (C3 for Simpler, the layer's root key \
                 for the multisampler) through the device's settings, and goes on until the \
                 same command comes with `down: false`, as the button's release. It needs the \
                 audio engine on; `sent` says whether it was. It changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<AuditionArgs>()
            .result::<Auditioned>(),
    )?;
    registry.register(
        CommandSpec::new(SET_SAMPLE_REGION, "Change Sample Region")
            .description(
                "Moves Simpler's Start and End flags, the part of the sample every mode plays \
                 (the sampler spec, R22). `start` and `end` are frames of the edited sample at \
                 its own rate; one left out stays where it is. An End past the sample's end \
                 comes back to it, and a Start at or after End is refused. The flags that \
                 cover the whole sample read back as no region. One undo step. The flags read \
                 back as `region` in the query tool's `device` result; the result here also \
                 holds the sample's `length`.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<SetSampleRegionArgs>()
            .result::<SampleRegionSet>(),
    )?;
    Ok(())
}
