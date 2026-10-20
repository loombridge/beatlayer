//! The Device View's state and commands (WP-2.4; the device chains spec):
//! which device is selected, which are folded, the device clipboard, and
//! hot-swapping, and the `device.*` commands that run on the shell (adding
//! a device as a person does, cut, copy, paste, duplicate, and presets);
//! and where floating devices' windows sit (WP-2.5b, IMP-14), which is
//! saved with the set but never an undo step.
//!
//! None of this is part of the set: selection, folding, the clipboard, and
//! hot-swapping are the view's, so they're never undo steps. The edits the
//! commands make go through the document's own commands, each command one
//! undo step named as Live 12.4.6 names it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bl_engine::DeviceType;
use bl_media::MediaStore;
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Chain, ChainId, Command, Device, DeviceDescription, DeviceId, DeviceKind, DeviceLayout,
    DevicePlace, DeviceRole, DeviceWindows, Document, Editor, LayerId, MultisamplerView,
    MultisamplerViews, ParamId, RackKind, RackViewMap, RackViews, TrackId, TrackKind,
};
use bl_project::preset::{self, Preset};
use bl_project::rack_preset::Saved;
use bl_shell_spec::arrangement as view_ids;
use bl_shell_spec::devices::{
    self as ids, AddDeviceArgs, AddPresetArgs, ChainAdded, ChainArgs, ChainRow, ChainSelect,
    CreateChainArgs, DeleteChainsArgs, DeviceAdded, DeviceArgs, DeviceFrame, DeviceListEntry,
    DevicePanel, DeviceParameters, DeviceViewState, DropChainOnTrackArgs, ExpandDeviceArgs,
    ExternalSection, FloatingWindow, FloatingWindows, FoldDeviceArgs, GroupArgs, Grouped, HotSwap,
    HotSwapArgs, HotSwapChainArgs, LoadChainPresetArgs, LoadPresetArgs, MapModeArgs,
    MidiSidechainSection, ParameterState, PasteChainArgs, PasteDeviceArgs, PresetResult, RackFrame,
    RackMacrosArgs, SaveChainPresetArgs, SavePresetArgs, SelectChainArgs, SelectDeviceArgs,
    SelectMacroArgs, SelectSlotArgs, SelectVariationArgs, SetAutoSelectArgs, SetMacroCountArgs,
    SetRackViewArgs, SetWindowArgs, SetZoneEditorArgs, ShowSidechainArgs, ShowVariationsArgs,
    SidechainSection, SoloChainArgs, Ungrouped,
};
use bl_shell_spec::devices::{ArmMapArgs, ArmedMap, MappingRow, ModulatorFrame};
use bl_shell_spec::multisampler::MultisamplerState;
use bl_shell_spec::pads as pad_ids;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::arrangement::ArrangementView;
use crate::browser::arrange::MediaFile;

mod rack_presets;

use rack_presets::Ready;
pub use rack_presets::is_chain_preset;

mod drum;
pub use drum::PadSample;

mod macros;

/// What the Device View shows besides the set: the selected device, the
/// folded devices, the clipboard, and hot-swapping; and the device types
/// the host offers, whose descriptions the generic UI draws (WP-2.5).
#[derive(Clone, Debug, Default)]
pub struct DeviceView {
    selected: Option<DeviceId>,
    folded: BTreeSet<DeviceId>,
    /// Devices whose expanded part is shown (WP-5.42).
    expanded: BTreeSet<DeviceId>,
    clipboard: Option<Device>,
    /// The chains cut or copied from a rack (WP-5.1).
    chain_clipboard: Vec<Chain>,
    hot_swap: Option<DeviceId>,
    /// The chain armed for hot-swap by its Hot-Swap button (WP-5.2).
    hot_swap_chain: Option<ChainId>,
    /// The chains selected in racks' chain lists, and the one clicked last,
    /// where a Shift-click's range starts (WP-5.2).
    chains_selected: BTreeSet<ChainId>,
    chain_anchor: Option<ChainId>,
    /// Whether a chain list has the keyboard focus, so S solos the selected
    /// chain and the edit keys act on chains (the `rack-chains` context).
    chain_list_focus: bool,
    /// The sample slot each device with slots shows the controls of
    /// (WP-5.17); a device without an entry shows slot 0.
    slots: std::collections::BTreeMap<DeviceId, u32>,
    /// Devices whose sidechain section is shown (WP-2.19b).
    sidechains_shown: BTreeSet<DeviceId>,
    /// The lit Map button, if one is (WP-5.10).
    armed: Option<(DeviceId, u8)>,
    /// The rack whose Map mode is on, if one is (WP-5.4): view state, never
    /// saved and never an undo step.
    map_mode: Option<DeviceId>,
    /// The macro selected in a rack's Macro Controls, and the mapping
    /// selected in its mappings list: view state (WP-5.4).
    macro_selected: Option<(DeviceId, u8)>,
    mapping_selected: Option<(DeviceId, bl_model::MacroTarget)>,
    /// The variation selected in a rack's Macro Variations view: view state
    /// (WP-5.5).
    variation_selected: Option<(DeviceId, usize)>,
    /// The view each device's display shows, where it has chosen one
    /// (WP-5.43).
    display_views: std::collections::BTreeMap<DeviceId, String>,
    /// The display data the engine's nodes last published (WP-5.43).
    displays: std::collections::BTreeMap<DeviceId, bl_engine::DisplayData>,
    types: Vec<DeviceType>,
    /// Where floating devices' windows sit (WP-2.5b). A frame outlives
    /// its device's docking and deletion, so undoing either puts the
    /// window back where it was; a save keeps only those of devices the
    /// set has.
    windows: DeviceWindows,
    /// Which views each rack shows (WP-5.2): saved with the set but not part
    /// of the document, so showing and hiding them is no undo step, and a
    /// rack that undo brings back shows what it did. A save keeps only those
    /// of racks the set has.
    rack_views: RackViewMap,
    /// Which parameter each knob of a plug-in device's panel shows
    /// (WP-2.10, D19), by `format/plugin_id`: absent entries keep the
    /// plug-in's own parameter at that place. Synced from the preferences.
    plugin_panels: std::collections::BTreeMap<String, std::collections::BTreeMap<usize, ParamId>>,
    /// The devices in Plug-In Configure Mode (WP-5.40, clap-hosting R5):
    /// their Configure switch is lit, and a parameter moved in their editor
    /// joins their configured list. View state, never saved with the set.
    configuring: BTreeSet<DeviceId>,
    /// The parameter each plug-in device's editor moved last without it being
    /// configured (manual 24.1.2): a temporary entry the automation and
    /// clip-envelope choosers list, dropped when another parameter moves.
    pending_params: std::collections::BTreeMap<DeviceId, ParamId>,
    /// What each multisampler's views show (WP-5.13): saved with the set,
    /// never an undo step.
    multisamplers: MultisamplerViews,
    /// Each multisampler's selected layers, in the layer list (R11).
    selections: std::collections::BTreeMap<DeviceId, Vec<LayerId>>,
    /// The multisampler layer whose sample the browser's next load replaces
    /// (WP-5.13, R10).
    layer_hot_swap: Option<(DeviceId, LayerId)>,
    /// With Auto Select on, the layers each held key plays (R7): while a
    /// note is held they are the selection.
    held: std::collections::BTreeMap<DeviceId, std::collections::BTreeMap<u8, Vec<LayerId>>>,
    /// The pad selected in each drum rack (WP-5.6, R13).
    pads_selected: std::collections::BTreeMap<DeviceId, u8>,
    /// The pad the browser's next load lands on, with its drum rack (R18).
    hot_swap_pad: Option<(DeviceId, u8)>,
    /// The drum chains whose Preview button is held (R5).
    previewing: BTreeSet<ChainId>,
    /// The chain Save as Default Pad kept (R17).
    default_pad: Option<Chain>,
    /// How many notes had reached each drum chain when Auto Select last
    /// looked (R20).
    hit_counts: std::collections::BTreeMap<ChainId, u64>,
}

/// Why a device command didn't run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceError {
    /// The arguments don't fit.
    InvalidArgs(String),
    /// It can't run now, and why.
    Unavailable(String),
    /// It ran and failed, and why; nothing changed.
    Failed(String),
}

/// What a device command did, for the shell to carry out.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// The command's result.
    pub value: Value,
    /// Whether the set changed.
    pub document_changed: bool,
    /// Whether only view state that the set keeps changed (which views a
    /// rack shows): the set needs saving, and nothing else follows.
    pub view_changed: bool,
    /// A line for the Status Bar.
    pub message: Option<String>,
    /// Preview buttons pressed (true) and released (false), which the shell
    /// tells the engine (the drum racks spec, R5).
    pub previews: Vec<(ChainId, bool)>,
    /// Folders for the browser to open, outermost first, with the User
    /// Library's label chosen: hot-swapping's way to the device's presets.
    pub browse: Option<Vec<PathBuf>>,
    /// Files the samples of a rack or chain preset were written to or found
    /// in, for the set's media table (WP-5.8).
    pub media: Vec<MediaFile>,
}

/// What a device command needs from the rest of the shell.
pub struct Context<'a> {
    /// The set.
    pub editor: &'a mut Editor,
    /// The Arrangement View, whose first selected track is the one the
    /// Device View shows.
    pub arrangement: &'a mut ArrangementView,
    /// The User Library, where presets go.
    pub user_library: Option<&'a Path>,
    /// The media store, where the samples of rack presets are found and
    /// registered.
    pub media: &'a MediaStore,
    /// The interface zoom, as a factor: a floating window's smallest size
    /// in points on the screen is its size at 100% times this.
    pub zoom: f32,
    /// Whether Auto-Hide Plug-In Windows is on.
    pub auto_hide: bool,
    /// Whether View ▸ Plug-In Windows shows the windows.
    pub windows_shown: bool,
}

/// The display of a device whose type draws none.
fn empty_display(device: DeviceId) -> bl_model::DeviceDisplay {
    bl_model::DeviceDisplay {
        device,
        live: false,
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, DeviceError> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| DeviceError::InvalidArgs(e.to_string()))
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// The track the Device View shows: the first selected track.
#[must_use]
pub fn shown_track(arrangement: &ArrangementView, doc: &Document) -> Option<TrackId> {
    arrangement
        .selected_tracks()
        .iter()
        .copied()
        .find(|track| doc.track(*track).is_some())
}

/// Where a new device with `role` goes in `track`'s chain when no index is
/// given: straight after `after` if that keeps the chain in order, or else
/// at the end of its role's place (the device chains spec, R12).
#[must_use]
pub fn default_index(
    doc: &Document,
    track: TrackId,
    role: DeviceRole,
    after: Option<DeviceId>,
) -> usize {
    let Some(track) = doc.track(track) else {
        return 0;
    };
    let roles: Vec<DeviceRole> = track.devices().map(|d| d.role).collect();
    index_among(&roles, role, after.and_then(|d| track.device_position(d)))
}

/// Where a device with `role` goes among a chain's devices, whose roles are
/// `roles`: straight after the one at `after` if that keeps the chain in
/// order, or else at the end of its role's place.
fn index_among(roles: &[DeviceRole], role: DeviceRole, after: Option<usize>) -> usize {
    let fits = |index: usize| {
        roles[..index].iter().all(|r| *r <= role) && roles[index..].iter().all(|r| *r >= role)
    };
    if let Some(index) = after.map(|i| i + 1)
        && fits(index)
    {
        return index;
    }
    roles.iter().take_while(|r| **r <= role).count()
}

impl DeviceView {
    /// Nothing selected, folded, copied, or hot-swapped.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The selected device.
    #[must_use]
    pub fn selected(&self) -> Option<DeviceId> {
        self.selected
    }

    /// Selects `device` without changing which track the Device View
    /// shows, as bringing its floating window to the front does (WP-2.5b).
    pub fn select_only(&mut self, device: DeviceId) {
        self.choose(Some(device));
    }

    /// Chooses the device the Device View shows as selected, ending a macro's
    /// or a variation's selection with it (macros spec, R14, R29): Rename,
    /// Duplicate and Delete act on one selection, so choosing a device takes
    /// them back from the macro or the variation they acted on.
    fn choose(&mut self, device: Option<DeviceId>) {
        self.selected = device;
        self.macro_selected = None;
        self.variation_selected = None;
    }

    /// Whether a device is folded.
    #[must_use]
    pub fn is_folded(&self, device: DeviceId) -> bool {
        self.folded.contains(&device)
    }

    /// The device being hot-swapped.
    #[must_use]
    pub fn hot_swap(&self) -> Option<DeviceId> {
        self.hot_swap
    }

    /// The chain armed by its Hot-Swap button, if one is.
    #[must_use]
    pub fn hot_swap_chain(&self) -> Option<ChainId> {
        self.hot_swap_chain
    }

    /// Whether the clipboard holds a device.
    #[must_use]
    pub fn can_paste(&self) -> bool {
        self.clipboard.is_some()
    }

    /// Sets the device types the host's engine makes, with their
    /// descriptions (WP-2.5).
    pub fn set_device_types(&mut self, types: Vec<DeviceType>) {
        self.types = types;
    }

    /// The extra audio outputs devices of `kind` offer past their main one,
    /// by the names their type gives them (WP-5.45); empty for a type that
    /// offers none, or one the host hasn't met.
    #[must_use]
    pub fn outputs(&self, kind: &DeviceKind) -> &[String] {
        self.types
            .iter()
            .find(|t| t.kind == *kind)
            .map_or(&[], |t| &t.outputs)
    }

    /// The description of devices of `kind`, if their type has one.
    #[must_use]
    pub fn description(&self, kind: &DeviceKind) -> Option<&DeviceDescription> {
        if matches!(kind, DeviceKind::Rack { .. }) {
            return Some(bl_model::rack_description());
        }
        self.types
            .iter()
            .find(|t| t.kind == *kind)
            .map(|t| &t.description)
            .filter(|d| !d.params.is_empty())
    }

    /// What the generic UI draws for `device`, if its type is described.
    /// A plug-in device gets the plug-in panel (WP-2.10, D19): up to
    /// [`PANEL_KNOBS`](bl_shell_spec::plugins::PANEL_KNOBS) knobs, mapped to
    /// the plug-in's first parameters or to the preferences' remap, with
    /// the rest of the body an open-editor target.
    fn panel(&self, device: &Device) -> Option<DevicePanel> {
        // A rack's controls are its frame's, not a panel's.
        if matches!(device.kind, DeviceKind::Rack { .. }) {
            return None;
        }
        let description = self.description(&device.kind)?;
        if let Some(key) = Self::plugin_key(&device.kind) {
            return Some(self.plugin_panel(device, description, &device.params, &key));
        }
        let mut shown = description.clone();
        if Self::is_multisampler(device) {
            // Spec: multisampler R1. The tab that shows has its controls.
            shown.layout = bl_shell_spec::multisampler::tab_layout(
                &shown.layout,
                self.multisampler_view(device.id).tab,
            );
        }
        Some(DevicePanel {
            values: description
                .params
                .iter()
                .map(|p| description.value(&device.params, p.id).unwrap_or(p.default))
                .collect(),
            description: shown,
            plugin: false,
            configured: Vec::new(),
            knobs: Vec::new(),
            selected_slot: self.slot_of(device),
            loaded_slots: device.media.iter().map(Option::is_some).collect(),
            // shortcut: every device with slots plays them on the keys the
            // eight-slot drum sampler does; a device with another mapping
            // names its keys in its own description.
            slot_keys: bl_devices::kitbag::KEYS
                .iter()
                .take(description.layout.slot_count() as usize)
                .map(|&key| bl_midi_io::note_name(key))
                .collect(),
        })
    }

    /// How many sample slots devices of `kind` have: 0 for a type with
    /// none.
    #[must_use]
    pub fn slot_count(&self, kind: &DeviceKind) -> u32 {
        self.description(kind).map_or(0, |d| d.layout.slot_count())
    }

    /// The sample slot `device`'s panel shows the controls of: the one
    /// selected, kept inside the slots its layout has.
    #[must_use]
    pub fn slot_of(&self, device: &Device) -> u32 {
        let count = self.slot_count(&device.kind);
        self.slots
            .get(&device.id)
            .copied()
            .unwrap_or(0)
            .min(count.saturating_sub(1))
    }

    /// A plug-in device's panel (WP-2.10, D19): the first parameters of the
    /// device's configured list as knobs (WP-5.40, clap-hosting R5), each a
    /// per-plug-in remap where the preferences set one.
    fn plugin_panel(
        &self,
        device: &Device,
        description: &DeviceDescription,
        values: &bl_model::OrdMap<ParamId, f64>,
        key: &str,
    ) -> DevicePanel {
        let all: Vec<ParamId> = description.params.iter().map(|p| p.id).collect();
        let configured = device.configured(&all);
        let remap = self.plugin_panels.get(key);
        let mapped = |index: usize| -> Option<&bl_model::ParamDescriptor> {
            remap
                .and_then(|m| m.get(&index))
                .and_then(|id| description.param(*id))
                .or_else(|| configured.get(index).and_then(|id| description.param(*id)))
        };
        let selected: Vec<bl_model::ParamDescriptor> = (0..bl_shell_spec::plugins::PANEL_KNOBS)
            .filter_map(mapped)
            .cloned()
            .collect();
        // The knob each drawn parameter sits on, 0 to 7 (WP-5.40): a knob
        // past the configured list's end keeps its own index, so its
        // context menu remaps that knob and not a neighbour.
        let knobs: Vec<usize> = (0..bl_shell_spec::plugins::PANEL_KNOBS)
            .filter(|index| mapped(*index).is_some())
            .collect();
        let knob_params: Vec<bl_shell_spec::devices::ConfiguredParam> = configured
            .iter()
            .filter_map(|id| description.param(*id))
            .map(|p| bl_shell_spec::devices::ConfiguredParam {
                param: p.id,
                name: p.name.clone(),
            })
            .collect();
        let controls: Vec<(ParamId, Widget)> =
            selected.iter().map(|p| (p.id, Widget::Knob)).collect();
        let layout = DeviceLayout::new(LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &controls,
        )));
        DevicePanel {
            values: selected
                .iter()
                .map(|p| description.value(values, p.id).unwrap_or(p.default))
                .collect(),
            description: DeviceDescription {
                params: selected,
                layout,
                views: Vec::new(),
                menu_options: Vec::new(),
            },
            plugin: true,
            configured: knob_params,
            knobs,
            selected_slot: 0,
            loaded_slots: Vec::new(),
            slot_keys: Vec::new(),
        }
    }

    /// The parameters a plug-in device's panel and the automation and
    /// clip-envelope choosers show (WP-5.40, clap-hosting R5): its configured
    /// list, in order, then the parameter its editor moved last if the device
    /// hasn't configured it (the temporary entry, manual 24.1.2). A device
    /// that isn't a plug-in gives every parameter.
    #[must_use]
    pub fn plugin_configured(
        &self,
        device: &Device,
        description: &DeviceDescription,
    ) -> Vec<ParamId> {
        let all: Vec<ParamId> = description.params.iter().map(|p| p.id).collect();
        if !matches!(device.kind, DeviceKind::Plugin { .. }) {
            return all;
        }
        let mut list: Vec<ParamId> = device.configured(&all).into_owned();
        if let Some(pending) = self.pending_params.get(&device.id)
            && all.contains(pending)
            && !list.contains(pending)
        {
            list.push(*pending);
        }
        list
    }

    /// Whether `device` is in Plug-In Configure Mode (WP-5.40).
    #[must_use]
    pub fn configuring(&self, device: DeviceId) -> bool {
        self.configuring.contains(&device)
    }

    /// Turns Plug-In Configure Mode on or off for `device`.
    pub fn set_configuring(&mut self, device: DeviceId, on: bool) {
        if on {
            self.configuring.insert(device);
        } else {
            self.configuring.remove(&device);
        }
    }

    /// Notes that a plug-in device's editor moved `param` without it being
    /// configured: its temporary chooser entry (manual 24.1.2), which
    /// replaces the one that parameter moved before it made.
    pub fn note_pending_param(&mut self, device: DeviceId, param: ParamId) {
        self.pending_params.insert(device, param);
    }

    /// Drops a plug-in device's temporary chooser entry, once its parameter
    /// is configured or the choosers stop offering it.
    pub fn clear_pending_param(&mut self, device: DeviceId) {
        self.pending_params.remove(&device);
    }

    /// `device`'s parameters as `device.parameters` reports them, if the set
    /// has that device.
    #[must_use]
    pub fn parameters_of(&self, doc: &Document, device: DeviceId) -> Option<DeviceParameters> {
        doc.device(device).map(|d| self.parameters(d))
    }

    /// `device`'s parameters as `device.parameters` reports them.
    fn parameters(&self, device: &Device) -> DeviceParameters {
        // A plug-in's panel shows its configured parameters (WP-5.40); any
        // other device's panel shows them all.
        let configured: BTreeSet<ParamId> = self
            .description(&device.kind)
            .map(|description| {
                let all: Vec<ParamId> = description.params.iter().map(|p| p.id).collect();
                if matches!(device.kind, DeviceKind::Plugin { .. }) {
                    device.configured(&all).iter().copied().collect()
                } else {
                    all.into_iter().collect()
                }
            })
            .unwrap_or_default();
        let params = self
            .description(&device.kind)
            .map(|description| {
                description
                    .params
                    .iter()
                    .map(|p| {
                        let value = description.value(&device.params, p.id).unwrap_or(p.default);
                        ParameterState {
                            descriptor: p.clone(),
                            value,
                            text: p.format(value),
                            menu_option: description.menu_options.contains(&p.id),
                            configured: configured.contains(&p.id),
                            other: device.can_compare().then(|| {
                                description
                                    .value(device.other_params(), p.id)
                                    .unwrap_or(p.default)
                            }),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        DeviceParameters {
            device: device.id,
            active: device
                .can_compare()
                .then(|| if device.b_active { "B" } else { "A" }.to_owned()),
            params,
        }
    }

    /// `params` as a device of `kind` takes them: clamped to its parameters'
    /// ranges, unknown ones left out, if its type is described.
    fn sanitize(
        &self,
        kind: &DeviceKind,
        params: bl_model::OrdMap<bl_model::ParamId, f64>,
    ) -> bl_model::OrdMap<bl_model::ParamId, f64> {
        match self.description(kind) {
            Some(description) => description.sanitize(&params),
            None => params,
        }
    }

    /// Forgets devices the set no longer has.
    pub fn forget_missing(&mut self, doc: &Document) {
        let exists = |d: &DeviceId| doc.device(*d).is_some();
        self.selected = self.selected.filter(exists);
        // Spec: macros R3, R14. A rack that's gone takes its Map mode and a
        // macro selected in it with it, so the browser comes back and Rename
        // names a device again.
        self.map_mode = self.map_mode.filter(exists);
        self.macro_selected = self.macro_selected.filter(|(rack, _)| exists(rack));
        // Spec: macros R29. A variation selected in a rack that's gone goes
        // with it.
        self.variation_selected = self.variation_selected.filter(|(rack, index)| {
            doc.device(*rack)
                .is_some_and(|device| *index < device.macros.variations.len())
        });
        self.hot_swap = self.hot_swap.filter(exists);
        self.slots.retain(|device, _| exists(device));
        self.folded.retain(exists);
        self.expanded.retain(exists);
        self.configuring.retain(exists);
        self.pending_params.retain(|device, _| exists(device));
        let chain_exists = |c: &ChainId| doc.chain(*c).is_some();
        self.chains_selected.retain(chain_exists);
        self.chain_anchor = self.chain_anchor.filter(chain_exists);
        self.hot_swap_chain = self.hot_swap_chain.filter(chain_exists);
        self.chain_list_focus &= self.chain_anchor.is_some();
        self.sidechains_shown.retain(exists);
        self.selections.retain(|device, _| exists(device));
        self.held.retain(|device, _| exists(device));
        self.layer_hot_swap = self.layer_hot_swap.filter(|(device, _)| exists(device));
        self.armed = self.armed.filter(|(d, _)| exists(d));
        self.display_views.retain(|d, _| exists(d));
        self.displays.retain(|d, _| exists(d));
        self.pads_selected.retain(|d, _| exists(d));
        self.hot_swap_pad = self.hot_swap_pad.filter(|(d, _)| exists(d));
        self.previewing.retain(chain_exists);
    }

    /// Whether `device`'s type has an expanded part.
    fn expandable(&self, device: &Device) -> bool {
        self.description(&device.kind)
            .is_some_and(|d| d.layout.has_expanded())
    }

    /// Whether a device's sidechain section is shown.
    #[must_use]
    pub fn is_sidechain_shown(&self, device: DeviceId) -> bool {
        self.sidechains_shown.contains(&device)
    }

    /// What the Device View shows for `track`.
    #[must_use]
    pub fn state(&self, doc: &Document, track: Option<TrackId>) -> DeviceViewState {
        let shown = track.and_then(|id| doc.track(id));
        let devices: Vec<DeviceFrame> = shown
            .map(|t| t.devices().map(|d| self.frame(doc, d)).collect())
            .unwrap_or_default();
        DeviceViewState {
            track: shown.map(bl_model::Track::id),
            track_kind: shown.map(bl_model::Track::kind),
            selected: devices.iter().find(|d| d.selected).map(|d| d.id),
            devices,
            hot_swap: self.hot_swap.and_then(|device| {
                doc.device(device).map(|d| HotSwap {
                    device,
                    label: if self.hot_swap_pad.is_some() {
                        "Swapping Pad".to_owned()
                    } else {
                        format!("Swapping {}", d.role.label())
                    },
                })
            }),
            can_paste: self.clipboard.is_some(),
            can_paste_chain: !self.chain_clipboard.is_empty(),
            device_list: shown
                .map(|t| {
                    let mut list = Vec::new();
                    Self::list_devices(t.devices(), 0, &mut list);
                    list
                })
                .unwrap_or_default(),
            armed_map: self.armed.map(|(device, slot)| ArmedMap { device, slot }),
            map_mode: self.map_mode,
        }
    }

    /// The lit Map button, if one is: its modulator and slot (WP-5.10).
    #[must_use]
    pub fn armed(&self) -> Option<(DeviceId, u8)> {
        self.armed
    }

    /// What a modulator's panel shows beside its parameters.
    fn modulator_frame(&self, doc: &Document, d: &Device) -> Option<ModulatorFrame> {
        use bl_model::modulator::{default_shape_of, has_shape_kind, is_modulator_kind};
        if !is_modulator_kind(&d.kind) {
            return None;
        }
        Some(ModulatorFrame {
            mappings: d
                .mappings
                .iter()
                .map(|m| MappingRow {
                    slot: m.slot,
                    target: m.target,
                    label: self.target_label(doc, m.target),
                })
                .collect(),
            armed: self
                .armed
                .filter(|(device, _)| *device == d.id)
                .map(|(_, slot)| slot),
            shape: has_shape_kind(&d.kind)
                .then(|| d.shape.clone().unwrap_or_else(|| default_shape_of(&d.kind))),
        })
    }

    /// How a mapping's slot names its target: a device's title and the
    /// parameter's name, or a track's label and its control.
    fn target_label(&self, doc: &Document, target: bl_model::modulator::ModTarget) -> String {
        use bl_model::MixerParam;
        use bl_model::modulator::ModTarget;
        match target {
            ModTarget::Device { device, param } => {
                doc.device(device).map_or_else(String::new, |d| {
                    let name = self
                        .description(&d.kind)
                        .and_then(|description| description.param(param))
                        .map_or_else(|| format!("Parameter {}", param.0), |p| p.name.clone());
                    format!("{} / {name}", d.title())
                })
            }
            ModTarget::Mixer { track, param } => format!(
                "{} {}",
                crate::routing::track_label(doc, track),
                match param {
                    MixerParam::Volume => "Volume",
                    MixerParam::Pan => "Pan",
                    MixerParam::Mute => "Track Activator",
                    MixerParam::SplitLeft => "Split Pan Left",
                    MixerParam::SplitRight => "Split Pan Right",
                }
            ),
            ModTarget::Send { track, to } => format!(
                "{} Send {}",
                crate::routing::track_label(doc, track),
                crate::routing::track_label(doc, to)
            ),
        }
    }

    /// The Device View selector's list of `devices` and what's inside them,
    /// as a hierarchy from `depth` (racks spec, R25).
    fn list_devices<'a>(
        devices: impl Iterator<Item = &'a Device>,
        depth: usize,
        list: &mut Vec<DeviceListEntry>,
    ) {
        for device in devices {
            list.push(DeviceListEntry {
                depth,
                label: device.title().to_owned(),
                device: Some(device.id),
                chain: None,
            });
            for chain in &device.chains {
                list.push(DeviceListEntry {
                    depth: depth + 1,
                    label: chain.name.clone(),
                    device: None,
                    chain: Some(chain.id),
                });
                Self::list_devices(chain.devices.iter(), depth + 2, list);
            }
        }
    }

    /// The chains selected in the chain list in focus (racks spec, R22).
    #[must_use]
    pub fn selected_chains(&self, doc: &Document) -> Vec<ChainId> {
        self.chain_targets(doc, None).unwrap_or_default()
    }

    /// Whether a chain list has the keyboard focus (the `rack-chains`
    /// context).
    #[must_use]
    pub fn chain_list_focus(&self) -> bool {
        self.chain_list_focus
    }

    /// The chains of `rack` that are selected, in the list's order: those
    /// clicked, or the first chain while none is (WP-5.2).
    fn rack_selection(&self, rack: &Device) -> Vec<ChainId> {
        let clicked: Vec<ChainId> = rack
            .chains
            .iter()
            .map(|c| c.id)
            .filter(|c| self.chains_selected.contains(c))
            .collect();
        if clicked.is_empty() {
            rack.chains.first().map(|c| c.id).into_iter().collect()
        } else {
            clicked
        }
    }

    /// The rack the chain list commands act on: the one whose chain was
    /// clicked last while a chain list has the focus, otherwise the selected
    /// device if it's a rack.
    fn focus_rack<'a>(&self, doc: &'a Document) -> Option<&'a Device> {
        self.chain_anchor
            .filter(|_| self.chain_list_focus)
            .and_then(|chain| doc.chain_rack(chain))
            .or_else(|| {
                self.selected
                    .and_then(|d| doc.device(d))
                    .filter(|d| d.rack_kind().is_some())
            })
    }

    /// The chains a chain command acts on: `chain`, or the selected chains
    /// of the rack in focus.
    fn chain_targets(
        &self,
        doc: &Document,
        chain: Option<ChainId>,
    ) -> Result<Vec<ChainId>, DeviceError> {
        if let Some(chain) = chain {
            return doc
                .chain(chain)
                .map(|c| vec![c.id])
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")));
        }
        let selected = self
            .focus_rack(doc)
            .map(|rack| self.rack_selection(rack))
            .unwrap_or_default();
        if selected.is_empty() {
            return Err(DeviceError::Unavailable("select a chain first".to_owned()));
        }
        Ok(selected)
    }

    /// A rack's frame (WP-5.2).
    fn rack_frame(&self, doc: &Document, rack: &Device, kind: RackKind) -> RackFrame {
        let selected = self.rack_selection(rack);
        let devices = match selected.as_slice() {
            [one] => rack
                .chains
                .iter()
                .find(|c| c.id == *one)
                .map(|c| c.devices.iter().map(|d| self.frame(doc, d)).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        RackFrame {
            kind,
            views: self.views_of(rack.id),
            chains: rack
                .chains
                .iter()
                .map(|c| {
                    let mut row = ChainRow {
                        id: c.id,
                        name: c.name.clone(),
                        colour: c.colour,
                        enabled: c.mixer.enabled,
                        solo: c.mixer.solo,
                        hot_swap: self.hot_swap_chain == Some(c.id),
                        volume: c.mixer.volume,
                        pan: c.mixer.pan,
                        has_mixer: kind.chains_have_volume_and_pan(),
                        selected: selected.contains(&c.id),
                        zones: c.zones,
                        drum: None,
                        output: None,
                    };
                    self.fill_drum_row(doc, rack, c, &mut row);
                    row
                })
                .collect(),
            pads: (kind == RackKind::Drum).then(|| self.pad_frame(rack)),
            return_tracks: if kind == RackKind::Drum {
                Self::return_tracks(doc)
            } else {
                Vec::new()
            },
            can_add_return: kind == RackKind::Drum
                && rack.chains.iter().filter(|c| c.is_return).count() < bl_model::MAX_RETURN_CHAINS,
            note: (selected.len() > 1).then(|| format!("{} chains selected", selected.len())),
            chain_list_focused: self.chain_list_focus
                && self
                    .chain_anchor
                    .is_some_and(|a| rack.chains.iter().any(|c| c.id == a)),
            selected_chains: selected,
            devices,
            selector: rack
                .params
                .get(&bl_model::CHAIN_SELECTOR)
                .map_or(0, |v| v.round().clamp(0.0, 127.0) as u8),
            filters_midi_ctrl: rack
                .params
                .get(&bl_model::FILTER_MIDI_CTRL)
                .is_some_and(|v| *v >= 0.5),
            macros: self.macro_state(doc, rack.id, self.views_of(rack.id).macro_count),
        }
    }

    /// Where floating devices' windows sit, for the next save.
    #[must_use]
    pub fn windows(&self) -> &DeviceWindows {
        &self.windows
    }

    /// Takes the window frames an opened set saved, or none for a new set.
    pub fn set_windows(&mut self, windows: DeviceWindows) {
        self.windows = windows;
    }

    /// The rack a view command acts on: `rack`, or the one in focus, and
    /// its kind.
    fn rack_to_view(
        &self,
        doc: &Document,
        rack: Option<DeviceId>,
    ) -> Result<(DeviceId, RackKind), DeviceError> {
        let rack = rack
            .or_else(|| self.focus_rack(doc).map(|d| d.id))
            .ok_or_else(|| DeviceError::Unavailable("select a rack first".to_owned()))?;
        let kind = doc
            .device(rack)
            .and_then(Device::rack_kind)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("{rack} isn't a rack")))?;
        Ok((rack, kind))
    }

    /// Selects, in each rack whose Auto Select is on, the chains the engine
    /// found processing (racks spec, R37); a rack with none processing keeps
    /// its selection. Returns whether a selection changed.
    pub fn follow_processing(&mut self, doc: &Document, processing: &[ChainId]) -> bool {
        let mut changed = false;
        for (&rack, views) in &self.rack_views {
            if !views.auto_select {
                continue;
            }
            let Some(rack) = doc.device(rack) else {
                continue;
            };
            let chosen: Vec<ChainId> = rack
                .chains
                .iter()
                .map(|c| c.id)
                .filter(|c| processing.contains(c))
                .collect();
            let current: Vec<ChainId> = rack
                .chains
                .iter()
                .map(|c| c.id)
                .filter(|c| self.chains_selected.contains(c))
                .collect();
            if chosen.is_empty() || chosen == current {
                continue;
            }
            self.chains_selected
                .retain(|c| !rack.chains.iter().any(|r| r.id == *c));
            self.chains_selected.extend(chosen);
            changed = true;
        }
        changed
    }

    /// Which views each rack shows, for the next save.
    #[must_use]
    pub fn rack_views(&self) -> &RackViewMap {
        &self.rack_views
    }

    /// Takes the rack views an opened set saved, or none for a new set.
    pub fn set_rack_views(&mut self, views: RackViewMap) {
        self.rack_views = views;
    }

    /// The views `rack` shows: all of them unless it has been given others.
    #[must_use]
    pub fn views_of(&self, rack: DeviceId) -> RackViews {
        self.rack_views.get(&rack).copied().unwrap_or_default()
    }

    /// `rack`'s macros as `rack.macros` reports them.
    fn macros_state(&self, doc: &Document, rack: DeviceId) -> Value {
        let count = self.views_of(rack).macro_count;
        self.macro_state(doc, rack, count)
            .map_or(Value::Null, |state| to_value(&state))
    }

    /// `rack`'s macros as `rack.macros` reports them (macros spec, R19), for
    /// a host reading a rack without the Device View.
    #[must_use]
    pub fn macros_of(
        &self,
        doc: &Document,
        rack: DeviceId,
        count: u8,
    ) -> Option<bl_shell_spec::devices::MacroState> {
        self.macro_state(doc, rack, count)
    }

    /// The rack whose Map mode is on, if one is (macros spec, R3).
    #[must_use]
    pub fn map_mode(&self) -> Option<DeviceId> {
        self.map_mode
    }

    /// The macro selected in a rack's Macro Controls, if one is.
    #[must_use]
    pub fn macro_selected(&self) -> Option<(DeviceId, u8)> {
        self.macro_selected
    }

    /// Selects a macro in a rack's Macro Controls, or none.
    pub fn select_macro(&mut self, macro_selected: Option<(DeviceId, u8)>) {
        self.macro_selected = macro_selected;
        // Spec: macros R29. One selection at a time: choosing a macro ends a
        // variation selection, and the other way round.
        if macro_selected.is_some() {
            self.variation_selected = None;
        }
    }

    /// The variation selected in a rack's Macro Variations view, if one is
    /// (macros spec, R29).
    #[must_use]
    pub fn variation_selected(&self) -> Option<(DeviceId, usize)> {
        self.variation_selected
    }

    /// Selects a variation in a rack's Macro Variations view, or none.
    pub fn select_variation(&mut self, variation_selected: Option<(DeviceId, usize)>) {
        self.variation_selected = variation_selected;
        if variation_selected.is_some() {
            self.macro_selected = None;
        }
    }

    /// The mapping selected in a rack's mappings list, if one is.
    #[must_use]
    pub fn mapping_selected(&self) -> Option<(DeviceId, bl_model::MacroTarget)> {
        self.mapping_selected
    }

    /// Selects a mapping in a rack's mappings list, or none.
    pub fn select_mapping(&mut self, selected: Option<(DeviceId, bl_model::MacroTarget)>) {
        self.mapping_selected = selected;
    }

    /// The macro that holds `device`'s `param`, by name, if one does (macros
    /// spec, R11).
    #[must_use]
    pub fn macro_holding(
        &self,
        doc: &Document,
        device: DeviceId,
        param: ParamId,
    ) -> Option<String> {
        self.mapped_hold(doc, device, param)
    }

    /// Where a device's window sits, if one has been placed: a floating
    /// device's (WP-2.5b) or a plug-in device's editor window (WP-2.10).
    #[must_use]
    pub fn window_frame(&self, device: DeviceId) -> Option<bl_model::WindowFrame> {
        self.windows.get(&device).copied()
    }

    /// Records where a plug-in device's editor window sits (`plugin.set_window`,
    /// WP-2.10): the same frame store as a floating device's, so a save
    /// keeps it and it's never an undo step.
    pub fn set_editor_frame(&mut self, device: DeviceId, frame: bl_model::WindowFrame) {
        self.windows.insert(device, frame);
    }

    /// The parameter mapping a plug-in device's panel knobs use, from the
    /// preferences: absent entries keep the plug-in's own parameter at that
    /// place (WP-2.10, D19).
    pub fn set_plugin_panels(
        &mut self,
        panels: std::collections::BTreeMap<String, std::collections::BTreeMap<usize, ParamId>>,
    ) {
        self.plugin_panels = panels;
    }

    /// The preferences key of a plug-in device, `format/plugin_id`
    /// (WP-2.10, D19), or `None` for a device that isn't a plug-in.
    #[must_use]
    pub fn plugin_key(kind: &DeviceKind) -> Option<String> {
        match kind {
            DeviceKind::Plugin { format, plugin_id } => Some(format!(
                "{}/{}",
                bl_plugin_host::scan::format_name(*format),
                plugin_id
            )),
            _ => None,
        }
    }

    /// Every floating device's window, on any track, in the order of the
    /// tracks and their chains (`device.windows`). None shows while
    /// `all_shown` is off (View ▸ Plug-In Windows); with `auto_hide` on,
    /// only the windows of `shown`'s devices show (Auto-Hide Plug-In
    /// Windows; the device chains spec, R31 and R33).
    #[must_use]
    pub fn floating_windows(
        &self,
        doc: &Document,
        shown: Option<TrackId>,
        auto_hide: bool,
        all_shown: bool,
    ) -> FloatingWindows {
        let windows = doc
            .track_order()
            .into_iter()
            .filter_map(|id| doc.track(id))
            .flat_map(|track| track.devices().map(move |d| (track.id(), d)))
            .filter(|(_, d)| d.floating)
            .map(|(track, d)| {
                let mut view = self.frame(doc, d);
                // The window shows the device whole, whatever its strip.
                view.folded = false;
                let (min_width, min_height) = ids::window_min_size(&view);
                let (grows_across, grows_down) = ids::window_grows(&view);
                FloatingWindow {
                    device: d.id,
                    track,
                    title: format!("{}/{}", d.title(), crate::routing::track_label(doc, track)),
                    frame: self.windows.get(&d.id).copied(),
                    min_width,
                    min_height,
                    grows_across,
                    grows_down,
                    view,
                    shown: all_shown && (!auto_hide || shown == Some(track)),
                }
            })
            .collect();
        FloatingWindows { windows }
    }

    /// Whether `device` is a multisampler (WP-5.13).
    #[must_use]
    pub fn is_multisampler(device: &Device) -> bool {
        matches!(
            &device.kind,
            DeviceKind::Native { device_type } if device_type == bl_devices::multisampler::MULTISAMPLER
        )
    }

    /// What a multisampler's views show: a new device's until a command
    /// changes them.
    #[must_use]
    pub fn multisampler_view(&self, device: DeviceId) -> MultisamplerView {
        self.multisamplers.get(&device).copied().unwrap_or_default()
    }

    /// Changes what a multisampler's views show.
    pub fn set_multisampler_view(&mut self, device: DeviceId, view: MultisamplerView) {
        if view == MultisamplerView::default() {
            self.multisamplers.remove(&device);
        } else {
            self.multisamplers.insert(device, view);
        }
    }

    /// Every multisampler's views, for the next save.
    #[must_use]
    pub fn multisampler_views(&self) -> &MultisamplerViews {
        &self.multisamplers
    }

    /// Takes the views an opened set saved, or none for a new set.
    pub fn set_multisampler_views(&mut self, views: MultisamplerViews) {
        self.multisamplers = views;
        self.selections.clear();
        self.held.clear();
        self.layer_hot_swap = None;
    }

    /// A multisampler's selected layers that are still in it, in list
    /// order: the layers of the notes held while Auto Select is on, else the
    /// ones picked in the list.
    #[must_use]
    pub fn layer_selection(&self, doc: &Document, device: DeviceId) -> Vec<LayerId> {
        doc.device(device)
            .map_or_else(Vec::new, |d| self.layer_selection_of(d))
    }

    /// The layer a load from the browser replaces the sample of, if one is
    /// being hot-swapped.
    #[must_use]
    pub fn layer_hot_swap(&self) -> Option<(DeviceId, LayerId)> {
        self.layer_hot_swap
    }

    /// Starts or ends hot-swapping a layer's sample.
    pub fn set_layer_hot_swap(&mut self, layer: Option<(DeviceId, LayerId)>) {
        self.layer_hot_swap = layer;
    }

    /// Picks a multisampler's layers in the list.
    pub fn select_layers(&mut self, device: DeviceId, layers: Vec<LayerId>) {
        self.selections.insert(device, layers);
    }

    /// A key went down or up on the computer keyboard: with Auto Select on,
    /// a multisampler on `track` selects the layers the note plays for as
    /// long as it's held (R7). Plays nothing in the set.
    pub fn note(&mut self, doc: &Document, track: TrackId, key: u8, velocity: u8, down: bool) {
        // A key goes up on every track, in case the view moved while it was
        // held.
        if !down {
            for held in self.held.values_mut() {
                held.remove(&key);
            }
            return;
        }
        let Some(track) = doc.track(track) else {
            return;
        };
        for device in track.devices().filter(|d| Self::is_multisampler(d)) {
            let held = self.held.entry(device.id).or_default();
            if self
                .multisamplers
                .get(&device.id)
                .is_some_and(|v| v.auto_select)
            {
                let selector = device
                    .params
                    .get(&bl_devices::multisampler::SELECTOR)
                    .map_or(0, |v| *v as u8);
                held.insert(key, device.layers.matching(key, velocity, selector));
            }
        }
    }

    /// A multisampler's views and selection, as `device_view.state` reports
    /// them.
    fn multisampler_state(&self, d: &Device) -> Option<MultisamplerState> {
        Self::is_multisampler(d).then(|| MultisamplerState {
            view: self.multisampler_view(d.id),
            selection: self.layer_selection_of(d),
            layers: d.layers.list.len(),
            hot_swap: self
                .layer_hot_swap
                .filter(|(device, _)| *device == d.id)
                .map(|(_, layer)| layer),
        })
    }

    /// As [`layer_selection`](Self::layer_selection), for a device already
    /// in hand.
    fn layer_selection_of(&self, d: &Device) -> Vec<LayerId> {
        let held: std::collections::BTreeSet<LayerId> = self
            .held
            .get(&d.id)
            .into_iter()
            .flat_map(|keys| keys.values().flatten().copied())
            .collect();
        let picked = self.selections.get(&d.id);
        d.layers
            .list
            .iter()
            .map(|l| l.id)
            .filter(|id| {
                if held.is_empty() {
                    picked.is_some_and(|p| p.contains(id))
                } else {
                    held.contains(id)
                }
            })
            .collect()
    }

    /// One device as the Device View, or its floating window, shows it.
    fn frame(&self, doc: &Document, d: &Device) -> DeviceFrame {
        DeviceFrame {
            id: d.id,
            title: d.title().to_owned(),
            name: d.own_name().to_owned(),
            kind: d.kind.clone(),
            role: d.role,
            enabled: d.enabled,
            takes_sample: matches!(
                &d.kind,
                DeviceKind::Native { device_type } if bl_devices::plays_samples(device_type)
            ),
            swaps_to_snapshot: matches!(
                &d.kind,
                DeviceKind::Native { device_type } if device_type == bl_devices::DRUM_SAMPLER
            ),
            show_preset_name: d.show_preset_name,
            folded: self.folded.contains(&d.id),
            expandable: self.expandable(d),
            expanded: self.expandable(d) && self.expanded.contains(&d.id),
            floating: d.floating,
            b_active: d.b_active,
            configure: self.configuring.contains(&d.id),
            selected: self.selected == Some(d.id),
            hot_swap: self.hot_swap == Some(d.id),
            multisampler: self.multisampler_state(d),
            panel: self.panel(d),
            sidechain: d.sidechain.as_ref().map(|sidechain| {
                let (source, tap) = match &sidechain.input {
                    bl_model::Input::Track { track, tap, .. } => {
                        (crate::routing::track_label(doc, *track), Some(*tap))
                    }
                    _ => ("No Input".to_owned(), None),
                };
                SidechainSection {
                    shown: self.sidechains_shown.contains(&d.id),
                    enabled: sidechain.enabled,
                    source,
                    tap,
                    mix: sidechain.mix,
                    gain: sidechain.gain,
                    listen: sidechain.listen,
                }
            }),
            external: d.external.as_ref().map(|external| ExternalSection {
                port: external.port.clone(),
                channel: external.channel,
                audio_from: external.audio_from.label(),
                hardware_latency: external.hardware_latency,
            }),
            midi_sidechain: d
                .midi_sidechain
                .as_ref()
                .map(|sidechain| MidiSidechainSection {
                    shown: self.sidechains_shown.contains(&d.id),
                    source: sidechain.source.map_or_else(
                        || "No Input".to_owned(),
                        |track| crate::routing::track_label(doc, track),
                    ),
                }),
            display_view: self.display_view(d),
            rack: d.rack_kind().map(|kind| self.rack_frame(doc, d, kind)),
            modulator: self.modulator_frame(doc, d),
        }
    }

    /// The view `device`'s display shows: the one chosen, or the first its
    /// description offers; none for a display with one view.
    fn display_view(&self, device: &Device) -> Option<String> {
        let views = &self.description(&device.kind)?.views;
        self.display_views
            .get(&device.id)
            .filter(|view| views.contains(view))
            .or_else(|| views.first())
            .cloned()
    }

    /// The display data the engine published, to fill from.
    pub fn displays_mut(
        &mut self,
    ) -> &mut std::collections::BTreeMap<DeviceId, bl_engine::DisplayData> {
        &mut self.displays
    }

    /// What `device`'s display shows now: its type's display from the
    /// device's values and the newest data its node published.
    #[must_use]
    pub fn display(&self, device: &Device) -> bl_model::DeviceDisplay {
        let DeviceKind::Native { device_type } = &device.kind else {
            return empty_display(device.id);
        };
        let view = self.display_view(device);
        let data = self.displays.get(&device.id).map(|d| d.values.as_slice());
        let mut display = self
            .description(&device.kind)
            .and_then(|description| {
                bl_devices::display_for_view(
                    device_type,
                    device.id,
                    description,
                    &device.params,
                    data,
                    view.as_deref(),
                )
            })
            .unwrap_or_else(|| empty_display(device.id));
        display.view = view;
        display
    }

    /// The device a command acts on: `device`, or the selected one.
    fn target(&self, doc: &Document, device: Option<DeviceId>) -> Result<DeviceId, DeviceError> {
        let device = device
            .or(self.selected)
            .ok_or_else(|| DeviceError::Unavailable("select a device first".to_owned()))?;
        if doc.device(device).is_none() {
            return Err(DeviceError::InvalidArgs(format!("no {device}")));
        }
        Ok(device)
    }

    /// Whether a person can run `id` with `args` now.
    #[must_use]
    pub fn can_run(&self, id: &str, args: &Value, doc: &Document, track: Option<TrackId>) -> bool {
        let named = args
            .get("device")
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        let has_target = || self.target(doc, named).is_ok();
        match id {
            ids::CUT
            | ids::COPY
            | ids::SAVE_PRESET
            | ids::SAVE_DEFAULT_PRESET
            | ids::FOLD
            | ids::SELECT_SLOT => has_target(),
            ids::EXPAND => self
                .target(doc, named)
                .is_ok_and(|device| doc.device(device).is_some_and(|d| self.expandable(d))),
            ids::SHOW_SIDECHAIN => self.target(doc, named).is_ok_and(|device| {
                doc.device(device)
                    .is_some_and(|d| d.sidechain.is_some() || d.midi_sidechain.is_some())
            }),
            ids::DUPLICATE => {
                // Spec: macros R29. With a variation selected, Duplicate
                // copies it, whatever the device is.
                self.variation_selected().is_some()
                    || self.target(doc, named).is_ok_and(|device| {
                        doc.device(device)
                            .is_some_and(|d| d.role != DeviceRole::Instrument)
                    })
            }
            ids::HOT_SWAP => has_target() || (args.get("on") == Some(&Value::Bool(false))),
            ids::PASTE => self.clipboard.as_ref().is_some_and(|device| {
                track
                    .and_then(|t| doc.track(t))
                    .is_some_and(|t| device.role.fits(t.kind()))
            }),
            ids::GROUP => {
                let devices: Vec<DeviceId> = args
                    .get("devices")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                args.get("chains").is_some()
                    || (devices.is_empty()
                        && self.chain_list_focus
                        && self.chain_targets(doc, None).is_ok())
                    || (devices.is_empty()
                        && self.selected.is_some_and(|d| doc.device(d).is_some()))
                    || devices.iter().any(|d| doc.device(*d).is_some())
            }
            ids::UNGROUP => self.target(doc, named).is_ok_and(|device| {
                doc.device(device)
                    .is_some_and(|d| d.rack_kind().is_some() && d.chains.len() <= 1)
            }),
            ids::CHAIN_PASTE => {
                !self.chain_clipboard.is_empty()
                    && (args.get("rack").is_some() || self.focus_rack(doc).is_some())
            }
            ids::CHAIN_COPY
            | ids::CHAIN_CUT
            | ids::CHAIN_DUPLICATE
            | ids::CHAIN_SOLO
            | ids::CHAIN_DELETE => {
                let chain = args
                    .get("chain")
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                args.get("chains").is_some() || self.chain_targets(doc, chain).is_ok()
            }
            ids::CHAIN_SELECT => {
                let step = matches!(
                    args.get("mode").and_then(Value::as_str),
                    Some("previous" | "next")
                );
                if step {
                    self.focus_rack(doc).is_some_and(|r| !r.chains.is_empty())
                } else {
                    true
                }
            }
            ids::RACK_SET_VIEW => {
                let rack = args
                    .get("rack")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .and_then(|d| doc.device(d))
                    .or_else(|| self.focus_rack(doc))
                    .filter(|d| d.rack_kind().is_some());
                let view: Option<bl_model::RackView> = args
                    .get("view")
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                // A key that fixes `shown` only acts when it changes
                // something, so Minus falls back to Fold (racks spec, R20).
                rack.is_some_and(
                    |rack| match (view, args.get("shown").and_then(Value::as_bool)) {
                        (Some(view), Some(shown)) => self.views_of(rack.id).shown(view) != shown,
                        _ => true,
                    },
                )
            }
            ids::RACK_SET_ZONE_EDITOR | ids::RACK_SET_AUTO_SELECT => args
                .get("rack")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .and_then(|d| doc.device(d))
                .or_else(|| self.focus_rack(doc))
                .is_some_and(|d| d.rack_kind().is_some()),
            ids::RACK_SET_MACRO_COUNT
            | ids::RACK_MAP_MODE
            | ids::RACK_MACROS
            | ids::RACK_SELECT_MACRO
            | ids::RACK_SHOW_VARIATIONS
            | ids::RACK_SELECT_VARIATION => args
                .get("rack")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .and_then(|d| doc.device(d))
                .or_else(|| self.focus_rack(doc))
                .is_some_and(|d| d.rack_kind().is_some()),
            pad_ids::SWITCH_HOT_SWAP_TARGET => {
                self.hot_swap_pad.is_some()
                    || self.hot_swap.is_some_and(|rack| {
                        doc.device(rack)
                            .is_some_and(|d| d.rack_kind() == Some(RackKind::Drum))
                            && self.pads_selected.contains_key(&rack)
                    })
            }
            pad_ids::GROUP_TO_DRUM_RACK => {
                let devices: Vec<DeviceId> = args
                    .get("devices")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let devices = if devices.is_empty() {
                    self.selected.into_iter().collect()
                } else {
                    devices
                };
                !devices.is_empty()
                    && devices.iter().all(|d| {
                        doc.device_place(*d)
                            .and_then(|p| doc.track(p.track))
                            .is_some_and(|t| t.kind() == TrackKind::Midi)
                    })
            }
            ids::LOAD_PRESET => has_target() && args.get("path").is_some(),
            ids::PARAMETERS | ids::DISPLAY | ids::SET_DISPLAY_VIEW => has_target(),
            ids::ARM_MAP => self.target(doc, named).is_ok_and(|device| {
                doc.device(device)
                    .is_some_and(|d| bl_model::modulator::is_modulator_kind(&d.kind))
            }),
            ids::SET_WINDOW => named
                .and_then(|device| doc.device(device))
                .is_some_and(|d| d.floating),
            _ => true,
        }
    }

    /// Runs Device View command `id`.
    ///
    /// # Errors
    ///
    /// [`DeviceError`] saying why it didn't run; nothing has changed then.
    pub fn run(&mut self, id: &str, args: Value, ctx: Context<'_>) -> Result<Outcome, DeviceError> {
        let mut outcome = Outcome::default();
        let Context {
            editor,
            arrangement,
            user_library,
            media,
            zoom,
            auto_hide,
            windows_shown,
        } = ctx;
        let track = shown_track(arrangement, editor.document());
        let windows = |view: &Self, editor: &Editor| {
            view.floating_windows(editor.document(), track, auto_hide, windows_shown)
        };
        let state = |view: &Self, editor: &Editor, arrangement: &ArrangementView| {
            to_value(&view.state(
                editor.document(),
                shown_track(arrangement, editor.document()),
            ))
        };
        match id {
            ids::STATE => {
                parse::<bl_commands::NoArgs>(args)?;
            }
            ids::WINDOWS => {
                parse::<bl_commands::NoArgs>(args)?;
                outcome.value = to_value(&windows(self, editor));
                return Ok(outcome);
            }
            ids::SET_WINDOW => {
                let SetWindowArgs { device, mut frame } = parse(args)?;
                if !frame.is_valid() {
                    return Err(DeviceError::InvalidArgs(
                        "a window's position and size must be finite, and its size above 0"
                            .to_owned(),
                    ));
                }
                let open = windows(self, editor);
                let window = open
                    .windows
                    .iter()
                    .find(|w| w.device == device)
                    .ok_or_else(|| DeviceError::Unavailable(format!("{device} isn't floating")))?;
                // IMP-14: the docked size is the smallest, and a device
                // without a display that grows stays at it.
                let zoom = f64::from(zoom);
                let (least_w, least_h) = (
                    f64::from(window.min_width) * zoom,
                    f64::from(window.min_height) * zoom,
                );
                frame.width = if window.grows_across {
                    frame.width.max(least_w)
                } else {
                    least_w
                };
                frame.height = if window.grows_down {
                    frame.height.max(least_h)
                } else {
                    least_h
                };
                self.windows.insert(device, frame);
                outcome.value = to_value(&windows(self, editor));
                return Ok(outcome);
            }
            ids::SELECT => {
                let SelectDeviceArgs { device } = parse(args)?;
                if let Some(device) = device {
                    let owner = editor
                        .document()
                        .device_track(device)
                        .ok_or_else(|| DeviceError::InvalidArgs(format!("no {device}")))?;
                    if track != Some(owner) {
                        select_track(arrangement, editor.document(), owner)?;
                    }
                }
                self.choose(device);
                self.chain_list_focus = false;
                if let Some(device) = device {
                    outcome.view_changed = self.reveal(editor, device);
                }
            }
            ids::SELECT_SLOT => {
                let SelectSlotArgs { device, slot } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let count = editor
                    .document()
                    .device(device)
                    .and_then(|d| self.description(&d.kind))
                    .map_or(0, |d| d.layout.slot_count());
                if slot >= count {
                    return Err(DeviceError::InvalidArgs(format!(
                        "{device} has {count} sample slots, counted from 0; there is no slot {slot}"
                    )));
                }
                self.slots.insert(device, slot);
            }
            ids::FOLD => {
                let FoldDeviceArgs { device, folded } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let folded = folded.unwrap_or(!self.folded.contains(&device));
                if folded {
                    self.folded.insert(device);
                } else {
                    self.folded.remove(&device);
                }
            }
            ids::EXPAND => {
                let ExpandDeviceArgs { device, expanded } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                if editor
                    .document()
                    .device(device)
                    .is_none_or(|d| !self.expandable(d))
                {
                    return Err(DeviceError::Unavailable(format!(
                        "{device} has no expanded part"
                    )));
                }
                let expanded = expanded.unwrap_or(!self.expanded.contains(&device));
                if expanded {
                    self.expanded.insert(device);
                } else {
                    self.expanded.remove(&device);
                }
            }
            ids::ADD => {
                let args: AddDeviceArgs = parse(args)?;
                let ready = self.default_rack(editor, &args, user_library, media, &mut outcome);
                let added = self.add(&args, editor, arrangement, user_library, track, ready)?;
                if let Some(name) = &added.default_preset {
                    // Spec: device-chains R13, as Live's "Using Default
                    // Preset Auto Filter.adv."
                    outcome.message = Some(format!(
                        "Using Default Preset {name}.{}.",
                        preset::PRESET_EXTENSION
                    ));
                }
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::SHOW_SIDECHAIN => {
                let ShowSidechainArgs { device, shown } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                if editor
                    .document()
                    .device(device)
                    .is_none_or(|d| d.sidechain.is_none() && d.midi_sidechain.is_none())
                {
                    return Err(DeviceError::Unavailable(format!(
                        "{device} has no sidechain input"
                    )));
                }
                let shown = shown.unwrap_or(!self.sidechains_shown.contains(&device));
                if shown {
                    self.sidechains_shown.insert(device);
                } else {
                    self.sidechains_shown.remove(&device);
                }
            }
            ids::CUT => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let copy = editor.document().device(device).cloned();
                editor
                    .apply(Command::RemoveDevice { device })
                    .map_err(|e| DeviceError::Failed(e.to_string()))?;
                self.clipboard = copy;
                outcome.document_changed = true;
            }
            ids::COPY => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                self.clipboard = editor.document().device(device).cloned();
            }
            ids::PASTE => {
                let PasteDeviceArgs {
                    track: to,
                    chain,
                    before,
                } = parse(args)?;
                let copy = self
                    .clipboard
                    .clone()
                    .ok_or_else(|| DeviceError::Unavailable("nothing to paste".to_owned()))?;
                let doc = editor.document();
                let place = match (chain, before) {
                    (Some(chain), _) => {
                        let track = doc
                            .chain_track(chain)
                            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
                        // Spec: device-chains R15. Before the selected
                        // device, as Live pastes, if it's in this chain.
                        let at = before
                            .or(self.selected)
                            .and_then(|d| doc.device_place(d))
                            .filter(|p| p.chain == Some(chain));
                        DevicePlace {
                            track,
                            chain: Some(chain),
                            index: at.map_or_else(
                                || doc.chain(chain).map_or(0, |c| c.devices.len()),
                                |p| p.index,
                            ),
                        }
                    }
                    (None, Some(device)) => {
                        let place = doc
                            .device_place(device)
                            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {device}")))?;
                        if let Some(to) = to.filter(|to| *to != place.track) {
                            return Err(DeviceError::InvalidArgs(format!(
                                "{device} isn't on {to}"
                            )));
                        }
                        place
                    }
                    (None, None) => {
                        let to = to.or(track).ok_or_else(|| {
                            DeviceError::Unavailable("select a track first".to_owned())
                        })?;
                        let chain = doc
                            .track(to)
                            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {to}")))?;
                        match self
                            .selected
                            .and_then(|d| doc.device_place(d))
                            .filter(|p| p.track == to)
                        {
                            Some(place) => place,
                            None => DevicePlace {
                                track: to,
                                chain: None,
                                index: chain.devices().len(),
                            },
                        }
                    }
                };
                let added = self.insert_copy(editor, &copy, place, "Copy Device")?;
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::DUPLICATE => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let doc = editor.document();
                let copy = doc.device(device).cloned().expect("the target exists");
                let mut place = doc.device_place(device).expect("the target has a place");
                place.index += 1;
                let added = self.insert_copy(editor, &copy, place, "Duplicate Device")?;
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::SWAP_TO_SNAPSHOT => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let added = self.swap_to_snapshot(editor, device)?;
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::GROUP => {
                let mut args: GroupArgs = parse(args)?;
                if args.devices.is_none() && args.chains.is_none() && self.chain_list_focus {
                    // Spec: racks R22. With the chain list in focus, Group
                    // acts on the selected chains.
                    args.chains = Some(self.chain_targets(editor.document(), None)?);
                }
                let grouped = self.group(editor, args)?;
                self.chain_list_focus = false;
                outcome.document_changed = true;
                outcome.value = to_value(&grouped);
                return Ok(outcome);
            }
            ids::UNGROUP => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let ungrouped = self.ungroup(editor, device)?;
                outcome.document_changed = true;
                outcome.value = to_value(&ungrouped);
                return Ok(outcome);
            }
            ids::CHAIN_COPY | ids::CHAIN_CUT => {
                let ChainArgs { chain } = parse(args)?;
                let doc = editor.document();
                let targets = self.chain_targets(doc, chain)?;
                let copies: Vec<Chain> = targets
                    .iter()
                    .filter_map(|c| doc.chain(*c).cloned())
                    .collect();
                if id == ids::CHAIN_CUT {
                    self.delete_chains(editor, &targets)?;
                    outcome.document_changed = true;
                }
                self.chain_clipboard = copies;
            }
            ids::CHAIN_DELETE => {
                let DeleteChainsArgs { chains } = parse(args)?;
                let targets = match chains {
                    Some(chains) => chains,
                    None => self.chain_targets(editor.document(), None)?,
                };
                self.delete_chains(editor, &targets)?;
                outcome.document_changed = true;
            }
            ids::CHAIN_PASTE => {
                let PasteChainArgs { rack, index } = parse(args)?;
                if self.chain_clipboard.is_empty() {
                    return Err(DeviceError::Unavailable("no chain to paste".to_owned()));
                }
                let doc = editor.document();
                let rack = match rack {
                    Some(rack) => rack,
                    None => self.focus_rack(doc).map(|d| d.id).ok_or_else(|| {
                        DeviceError::Unavailable("select a rack first".to_owned())
                    })?,
                };
                let at = index
                    .unwrap_or_else(|| doc.device(rack).map_or(0, |device| device.chains.len()));
                let copies = self.chain_clipboard.clone();
                outcome.value = to_value(&self.insert_chain_copies(editor, &copies, rack, at)?);
                outcome.document_changed = true;
                return Ok(outcome);
            }
            ids::CHAIN_DUPLICATE => {
                let ChainArgs { chain } = parse(args)?;
                let targets = self.chain_targets(editor.document(), chain)?;
                outcome.value = to_value(&self.duplicate_chains(editor, &targets)?);
                outcome.document_changed = true;
                return Ok(outcome);
            }
            ids::CHAIN_SELECT => {
                let args: SelectChainArgs = parse(args)?;
                self.select_chain(editor.document(), args)?;
                if let (Some(chain), ChainSelect::Replace) = (args.chain, args.mode)
                    && let Some(rack) = editor.document().chain_rack(chain).map(|r| r.id)
                {
                    // Spec: racks R25. A chain chosen from the device list
                    // shows, with every rack around it.
                    self.folded.remove(&rack);
                    outcome.view_changed = self.reveal(editor, rack);
                    // Spec: drum-racks R13. The chain's pad is selected.
                    self.follow_chain_selection(editor.document(), chain);
                }
            }
            ids::CHAIN_SOLO => {
                let SoloChainArgs { chain, add } = parse(args)?;
                let targets = self.chain_targets(editor.document(), chain)?;
                self.solo_chains(editor, &targets, add)?;
                outcome.document_changed = true;
            }
            ids::CHAIN_HOT_SWAP => {
                let HotSwapChainArgs { chain, on } = parse(args)?;
                if editor.document().chain(chain).is_none() {
                    return Err(DeviceError::InvalidArgs(format!("no {chain}")));
                }
                if on.unwrap_or(self.hot_swap_chain != Some(chain)) {
                    self.hot_swap_chain = Some(chain);
                    self.hot_swap = None;
                    self.hot_swap_pad = None;
                } else if self.hot_swap_chain == Some(chain) {
                    self.hot_swap_chain = None;
                }
            }
            ids::CHAIN_CREATE => {
                let args: CreateChainArgs = parse(args)?;
                outcome.value = to_value(&self.create_chain(
                    editor,
                    args,
                    user_library,
                    media,
                    &mut outcome,
                )?);
                outcome.document_changed = true;
                return Ok(outcome);
            }
            ids::CHAIN_DROP_ON_TRACK => {
                let args: DropChainOnTrackArgs = parse(args)?;
                self.drop_chain_on_track(editor, &args)?;
                outcome.document_changed = true;
            }
            pad_ids::MOVE_PADS => {
                let args: pad_ids::MovePadsArgs = parse(args)?;
                self.move_pads(editor.document(), args)?;
                outcome.view_changed = true;
            }
            pad_ids::PAD_SELECT => {
                let args: pad_ids::PadArgs = parse(args)?;
                self.select_pad(editor.document(), args)?;
            }
            pad_ids::PAD_SET_MUTE | pad_ids::PAD_SET_SOLO => {
                let args: pad_ids::PadSwitchArgs = parse(args)?;
                self.set_pad_switch(editor, args, id == pad_ids::PAD_SET_SOLO)?;
                outcome.document_changed = true;
            }
            pad_ids::PAD_PREVIEW => {
                let pad_ids::PadPreviewArgs { rack, note, on } = parse(args)?;
                let chains = self.pad_chain_ids(editor.document(), rack, note)?;
                self.preview(editor.document(), &chains, on, &mut outcome)?;
            }
            pad_ids::CHAIN_PREVIEW => {
                let pad_ids::ChainPreviewArgs { chain, on } = parse(args)?;
                self.preview(editor.document(), &[chain], on, &mut outcome)?;
            }
            pad_ids::PAD_SWAP => {
                let args: pad_ids::PadSwapArgs = parse(args)?;
                self.swap_pads(editor, args)?;
                outcome.document_changed = true;
            }
            pad_ids::PAD_SAVE_DEFAULT => {
                let args: pad_ids::PadArgs = parse(args)?;
                self.save_default_pad(editor.document(), args, user_library)?;
            }
            pad_ids::PAD_DELETE => {
                let args: pad_ids::PadArgs = parse(args)?;
                let chains = self.pad_chain_ids(editor.document(), args.rack, args.note)?;
                if chains.is_empty() {
                    return Err(DeviceError::Unavailable(
                        "the pad holds no chain".to_owned(),
                    ));
                }
                self.delete_chains(editor, &chains)?;
                outcome.document_changed = true;
            }
            pad_ids::PAD_RENAME => {
                let args: pad_ids::PadRenameArgs = parse(args)?;
                self.rename_pad(editor, args)?;
                outcome.document_changed = true;
            }
            pad_ids::PAD_HOT_SWAP => {
                let args: pad_ids::PadHotSwapArgs = parse(args)?;
                self.hot_swap_pad(editor.document(), args)?;
            }
            pad_ids::SWITCH_HOT_SWAP_TARGET => {
                parse::<bl_commands::NoArgs>(args)?;
                self.switch_hot_swap_target(editor.document())?;
            }
            pad_ids::GROUP_TO_DRUM_RACK => {
                let args: pad_ids::GroupToDrumRackArgs = parse(args)?;
                outcome.value = to_value(&self.group_to_drum_rack(editor, args)?);
                outcome.document_changed = true;
                return Ok(outcome);
            }
            pad_ids::CHAIN_EXTRACT => {
                let args: pad_ids::ExtractChainArgs = parse(args)?;
                outcome.value = to_value(&self.extract_chain(editor, args)?);
                outcome.document_changed = true;
                return Ok(outcome);
            }
            pad_ids::CHAIN_EXTRACT_RETURN => {
                let args: pad_ids::ExtractChainArgs = parse(args)?;
                outcome.value = to_value(&self.extract_return(editor, args)?);
                outcome.document_changed = true;
                return Ok(outcome);
            }
            pad_ids::PAD_DROP | pad_ids::PAD_HOT_SWAP_SAMPLE => {
                return Err(DeviceError::Unavailable(format!(
                    "`{id}` needs the shell's media store"
                )));
            }
            ids::RACK_SET_VIEW => {
                let SetRackViewArgs { rack, view, shown } = parse(args)?;
                let doc = editor.document();
                let rack = rack
                    .or_else(|| self.focus_rack(doc).map(|d| d.id))
                    .ok_or_else(|| DeviceError::Unavailable("select a rack first".to_owned()))?;
                if doc.device(rack).is_none_or(|d| d.rack_kind().is_none()) {
                    return Err(DeviceError::InvalidArgs(format!("{rack} isn't a rack")));
                }
                let views = self.views_of(rack);
                let shown = shown.unwrap_or(!views.shown(view));
                self.rack_views.insert(rack, views.with(view, shown));
                // Spec: macros R29. Hiding the Macro Variations view takes the
                // selection of a variation in it with it.
                if view == bl_model::RackView::Variations && !shown {
                    self.variation_selected = self.variation_selected.filter(|(r, _)| *r != rack);
                }
                outcome.view_changed = true;
            }
            ids::RACK_SET_ZONE_EDITOR => {
                let SetZoneEditorArgs { rack, editor: zone } = parse(args)?;
                let (rack, kind) = self.rack_to_view(editor.document(), rack)?;
                if let Some(zone) = zone
                    && !kind.zone_kinds().contains(&zone)
                {
                    return Err(DeviceError::InvalidArgs(format!(
                        "{rack}'s chains have no {zone:?} zone"
                    )));
                }
                let mut views = self.views_of(rack);
                views.zone_editor = zone;
                self.rack_views.insert(rack, views);
                outcome.view_changed = true;
            }
            ids::RACK_SET_AUTO_SELECT => {
                let SetAutoSelectArgs { rack, on } = parse(args)?;
                let (rack, _) = self.rack_to_view(editor.document(), rack)?;
                let mut views = self.views_of(rack);
                views.auto_select = on.unwrap_or(!views.auto_select);
                self.rack_views.insert(rack, views);
                outcome.view_changed = true;
            }
            ids::RACK_SET_MACRO_COUNT => {
                let SetMacroCountArgs { rack, count } = parse(args)?;
                let (rack, _) = self.rack_to_view(editor.document(), rack)?;
                if !(1..=bl_model::MACROS).contains(&count) {
                    return Err(DeviceError::InvalidArgs(
                        "a rack shows 1 to 16 macros".to_owned(),
                    ));
                }
                let mut views = self.views_of(rack);
                views.macro_count = count;
                self.rack_views.insert(rack, views);
                outcome.view_changed = true;
                outcome.value = self.macros_state(editor.document(), rack);
                return Ok(outcome);
            }
            ids::RACK_MAP_MODE => {
                let MapModeArgs { rack, on } = parse(args)?;
                let (rack, _) = self.rack_to_view(editor.document(), rack)?;
                let on = on.unwrap_or(self.map_mode != Some(rack));
                self.map_mode = on.then_some(rack);
                outcome.value = self.macros_state(editor.document(), rack);
                return Ok(outcome);
            }
            ids::RACK_MACROS => {
                let RackMacrosArgs { rack } = parse(args)?;
                let (rack, _) = self.rack_to_view(editor.document(), rack)?;
                outcome.value = self.macros_state(editor.document(), rack);
                return Ok(outcome);
            }
            ids::RACK_SELECT_MACRO => {
                let SelectMacroArgs { rack, macro_number } = parse(args)?;
                let (rack, _) = self.rack_to_view(editor.document(), rack)?;
                if let Some(number) = macro_number
                    && !(1..=bl_model::MACROS).contains(&number)
                {
                    return Err(DeviceError::InvalidArgs(
                        "a rack's macros are numbered 1 to 16".to_owned(),
                    ));
                }
                self.select_macro(macro_number.map(|number| (rack, number)));
                outcome.value = self.macros_state(editor.document(), rack);
                return Ok(outcome);
            }
            ids::RACK_SHOW_VARIATIONS => {
                let ShowVariationsArgs { rack, shown } = parse(args)?;
                let (rack, _) = self.rack_to_view(editor.document(), rack)?;
                let views = self.views_of(rack);
                let shown = shown.unwrap_or(!views.shown(bl_model::RackView::Variations));
                self.rack_views
                    .insert(rack, views.with(bl_model::RackView::Variations, shown));
                if !shown {
                    self.variation_selected = self.variation_selected.filter(|(r, _)| *r != rack);
                }
                outcome.view_changed = true;
                outcome.value = self.macros_state(editor.document(), rack);
                return Ok(outcome);
            }
            ids::RACK_SELECT_VARIATION => {
                let SelectVariationArgs { rack, index } = parse(args)?;
                let (rack, _) = self.rack_to_view(editor.document(), rack)?;
                if let Some(index) = index
                    && index
                        >= editor
                            .document()
                            .device(rack)
                            .map_or(0, |d| d.macros.variations.len())
                {
                    return Err(DeviceError::InvalidArgs(
                        "no such macro variation".to_owned(),
                    ));
                }
                self.select_variation(index.map(|index| (rack, index)));
                outcome.value = self.macros_state(editor.document(), rack);
                return Ok(outcome);
            }
            ids::SAVE_PRESET => {
                let SavePresetArgs { device, path } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let current = editor.document().device(device).expect("the target exists");
                let path = match path {
                    Some(path) => preset_path(&path)?,
                    None => {
                        let library = user_library.ok_or_else(no_library)?;
                        preset::unused_path(
                            &preset::presets_folder(library, current),
                            current.title(),
                        )
                    }
                };
                rack_presets::save_device(current, media, &path)?;
                outcome.message = Some(format!("Saved preset {}.", path.display()));
                outcome.value = to_value(&preset_result(device, &path));
                return Ok(outcome);
            }
            ids::SAVE_DEFAULT_PRESET => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let library = user_library.ok_or_else(no_library)?;
                let current = editor.document().device(device).expect("the target exists");
                let path = preset::default_preset_path(library, current);
                rack_presets::save_device(current, media, &path)?;
                outcome.message = Some(format!(
                    "Saved the default preset for {}: {}.",
                    current.own_name(),
                    path.display()
                ));
                outcome.value = to_value(&preset_result(device, &path));
                return Ok(outcome);
            }
            ids::ARM_MAP => {
                let ArmMapArgs {
                    device,
                    slot,
                    armed,
                } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                if !editor
                    .document()
                    .device(device)
                    .is_some_and(|d| bl_model::modulator::is_modulator_kind(&d.kind))
                {
                    return Err(DeviceError::Unavailable(format!(
                        "{device} isn't a modulator"
                    )));
                }
                let slots = editor
                    .document()
                    .device(device)
                    .map_or(bl_model::modulator::SLOTS, |d| {
                        bl_model::modulator::slots_of(&d.kind)
                    });
                if slot >= slots {
                    return Err(DeviceError::InvalidArgs(format!(
                        "that modulator's slots are 0 to {}",
                        slots - 1
                    )));
                }
                let lit = self.armed == Some((device, slot));
                self.armed = if armed.unwrap_or(!lit) {
                    Some((device, slot))
                } else if lit {
                    None
                } else {
                    self.armed
                };
            }
            ids::DISPLAY => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let current = editor.document().device(device).expect("the target exists");
                outcome.value = to_value(&self.display(current));
                return Ok(outcome);
            }
            ids::SET_DISPLAY_VIEW => {
                let ids::SetDisplayViewArgs { device, view } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let current = editor.document().device(device).expect("the target exists");
                let offered = self
                    .description(&current.kind)
                    .map_or(&[][..], |d| d.views.as_slice());
                if !offered.contains(&view) {
                    return Err(DeviceError::InvalidArgs(format!(
                        "{device}'s display has no view {view:?}; it offers {offered:?}"
                    )));
                }
                self.display_views.insert(device, view);
            }
            ids::PARAMETERS => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let current = editor.document().device(device).expect("the target exists");
                outcome.value = to_value(&self.parameters(current));
                return Ok(outcome);
            }
            ids::LOAD_PRESET => {
                let LoadPresetArgs { device, path } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let path = preset_path(&path)?;
                let device =
                    match rack_presets::open_preset(&path, media, user_library, &mut outcome)? {
                        Saved::Device(_) => {
                            load_preset(self, editor, device, &path)?;
                            device
                        }
                        saved => self.load_rack_preset(
                            editor,
                            device,
                            saved,
                            &preset::preset_name(&path),
                        )?,
                    };
                outcome.document_changed = true;
                outcome.value = to_value(&preset_result(device, &path));
                return Ok(outcome);
            }
            ids::ADD_PRESET => {
                let args: AddPresetArgs = parse(args)?;
                let (add, ready) =
                    self.ready_to_add(&args, editor, user_library, media, &mut outcome)?;
                let added =
                    self.add(&add, editor, arrangement, user_library, track, Some(ready))?;
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::REPLACE => {
                let args: ids::ReplaceArgs = parse(args)?;
                let added = self.replace(args, editor, user_library, media, &mut outcome)?;
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::CHAIN_SAVE_PRESET => {
                let args: SaveChainPresetArgs = parse(args)?;
                let (saved, path) = self.save_chain_preset(args, editor, user_library, media)?;
                outcome.message = Some(format!("Saved preset {}.", path.display()));
                outcome.value = to_value(&saved);
                return Ok(outcome);
            }
            ids::CHAIN_LOAD_PRESET => {
                let args: LoadChainPresetArgs = parse(args)?;
                let added =
                    self.load_chain_preset(args, editor, user_library, media, &mut outcome)?;
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::HOT_SWAP => {
                let HotSwapArgs { device, on } = parse(args)?;
                let on = on.unwrap_or(
                    self.hot_swap.is_none() || device.is_some_and(|d| Some(d) != self.hot_swap),
                );
                self.hot_swap_pad = None;
                if on {
                    // Spec: racks R44. With none named or selected, the
                    // shown track's first audio effect or its instrument.
                    let named = device
                        .or(self.selected)
                        .or_else(|| Self::first_hot_swap_target(editor.document(), track));
                    let device = self.target(editor.document(), named)?;
                    self.hot_swap = Some(device);
                    self.hot_swap_chain = None;
                    if let Some(library) = user_library {
                        let current = editor.document().device(device).expect("the target exists");
                        outcome.browse = Some(presets_route(library, current));
                    }
                } else {
                    self.hot_swap = None;
                }
            }
            _ => {
                return Err(DeviceError::InvalidArgs(format!(
                    "`{id}` isn't a Device View command"
                )));
            }
        }
        outcome.value = state(self, editor, arrangement);
        Ok(outcome)
    }

    /// Replaces the Drum Sampler `device` with Simpler in One-Shot
    /// mode (`device.swap_to_snapshot`; one-shot drum sampler R22): the
    /// same place, sample and on or off state, and the same Start and
    /// Length, in one undo step.
    fn swap_to_snapshot(
        &mut self,
        editor: &mut Editor,
        device: DeviceId,
    ) -> Result<DeviceAdded, DeviceError> {
        use bl_devices::{drum_sampler, sampler};
        let doc = editor.document();
        let old = doc.device(device).expect("the target exists");
        let strike = drum_sampler::drum_sampler_type();
        if old.kind != strike.kind {
            return Err(DeviceError::Unavailable(format!(
                "{device} isn't a drum sampler"
            )));
        }
        let place = doc.device_place(device).expect("the target has a place");
        let kept = |id| {
            strike
                .description
                .value(&old.params, id)
                .expect("Drum Sampler describes it")
        };
        let (enabled, media) = (old.enabled, old.media.clone());
        let (start, length) = (kept(drum_sampler::START), kept(drum_sampler::LENGTH));
        let id: DeviceId = editor.new_id();
        let mut snapshot = sampler::sampler_type().device(id);
        snapshot.enabled = enabled;
        snapshot.media = media;
        snapshot.params.insert(sampler::MODE, 1.0);
        snapshot.params.insert(sampler::START, start);
        snapshot.params.insert(sampler::LENGTH, length);
        // A chain holds one instrument, so the old one goes first.
        editor
            .transact("Swap to Simpler", |editor| {
                editor.apply(Command::RemoveDevice { device })?;
                match place.chain {
                    None => editor.apply(Command::InsertDevice {
                        track: place.track,
                        index: place.index,
                        device: snapshot,
                    }),
                    Some(chain) => editor.apply(Command::InsertChainDevice {
                        chain,
                        index: place.index,
                        device: snapshot,
                    }),
                }
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.choose(Some(id));
        Ok(DeviceAdded {
            device: id,
            track: place.track,
            new_track: false,
            default_preset: None,
        })
    }

    /// Adds a device as a person does (`device.add`): a new one of the type
    /// `args` describe, or `ready`, a device a preset made.
    fn add(
        &mut self,
        args: &AddDeviceArgs,
        editor: &mut Editor,
        arrangement: &mut ArrangementView,
        user_library: Option<&Path>,
        shown: Option<TrackId>,
        ready: Option<Ready>,
    ) -> Result<DeviceAdded, DeviceError> {
        if let Some(chain) = args.chain {
            return self.add_to_chain(args, chain, editor, user_library, ready);
        }
        let role = args.device.role;
        let target = args
            .track
            .or(shown)
            .ok_or_else(|| DeviceError::Unavailable("select a track first".to_owned()))?;
        let kind = editor
            .document()
            .track(target)
            .map(bl_model::Track::kind)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {target}")))?;
        // Spec: device-chains R12. An instrument or MIDI effect for a
        // track that can't hold it goes on a new MIDI track at the end.
        let new_track = !role.fits(kind);
        let Ready {
            device,
            state,
            original,
            default_name,
        } = self.plan(editor, args, user_library, ready);
        let id = device.id;
        let copies: Vec<DeviceId> = device.walk().map(|d| d.id).collect();
        let pairs: Vec<(&Device, DeviceId)> =
            original.iter().flat_map(Device::walk).zip(copies).collect();
        let label = if new_track {
            "Insert Device and Track"
        } else {
            "Insert Device"
        };
        let track_id: TrackId = if new_track { editor.new_id() } else { target };
        let index = match (new_track, args.index) {
            (true, _) => 0,
            (false, Some(index)) => index,
            (false, None) => default_index(editor.document(), target, role, self.selected),
        };
        let result = editor.transact(label, |editor| {
            if new_track {
                let top = editor.document().top_level().len();
                editor.apply(Command::CreateTrack {
                    id: track_id,
                    kind: TrackKind::Midi,
                    name: format!("{} {}", top + 1, args.device.name),
                    parent: None,
                    index: top,
                })?;
            }
            editor.apply(Command::InsertDevice {
                track: track_id,
                index,
                device,
            })?;
            if let Some((params, name, shape)) = state {
                editor.apply(Command::SetDeviceState {
                    device: id,
                    params,
                    name,
                    shape,
                })?;
            }
            map_inner_copies(editor, &pairs)
        });
        result.map_err(|e| DeviceError::Failed(e.to_string()))?;
        if new_track {
            select_track(arrangement, editor.document(), track_id)?;
        }
        self.choose(Some(id));
        Ok(DeviceAdded {
            device: id,
            track: track_id,
            new_track,
            default_preset: default_name,
        })
    }

    /// The device `add` puts in: `ready`, or a new one of the type `args`
    /// describe with its default preset's values, if the User Library has
    /// one it fits (the device chains spec, R13).
    fn plan(
        &self,
        editor: &mut Editor,
        args: &AddDeviceArgs,
        user_library: Option<&Path>,
        ready: Option<Ready>,
    ) -> Ready {
        if let Some(ready) = ready {
            return ready;
        }
        let device = new_device(editor.new_id(), args, &self.types);
        let default = user_library
            .map(|library| preset::default_preset_path(library, &device))
            .filter(|path| path.is_file())
            .and_then(|path| {
                Preset::load(&path)
                    .ok()
                    .filter(|p| p.fits(&device))
                    .map(|p| (path, p))
            });
        let default_name = default.as_ref().map(|(path, _)| preset::preset_name(path));
        let state = default.map(|(path, preset)| {
            (
                self.sanitize(&args.device.kind, preset.params),
                preset::preset_name(&path),
                preset.shape,
            )
        });
        Ready {
            device,
            state,
            original: None,
            default_name,
        }
    }

    /// Adds a device to a rack's chain (`device.add` with `chain`), after
    /// the selected device if it's in the chain, with its default preset.
    fn add_to_chain(
        &mut self,
        args: &AddDeviceArgs,
        chain: ChainId,
        editor: &mut Editor,
        user_library: Option<&Path>,
        ready: Option<Ready>,
    ) -> Result<DeviceAdded, DeviceError> {
        let doc = editor.document();
        let track = doc
            .chain_track(chain)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
        let roles: Vec<DeviceRole> = doc
            .chain(chain)
            .map(|c| c.devices.iter().map(|d| d.role).collect())
            .unwrap_or_default();
        let after = self
            .selected
            .and_then(|d| doc.device_place(d))
            .filter(|p| p.chain == Some(chain))
            .map(|p| p.index);
        let index = args
            .index
            .unwrap_or_else(|| index_among(&roles, args.device.role, after));
        let Ready {
            device,
            state,
            original,
            default_name,
        } = self.plan(editor, args, user_library, ready);
        let id = device.id;
        let copies: Vec<DeviceId> = device.walk().map(|d| d.id).collect();
        let pairs: Vec<(&Device, DeviceId)> =
            original.iter().flat_map(Device::walk).zip(copies).collect();
        editor
            .transact("Insert Device", |editor| {
                editor.apply(Command::InsertChainDevice {
                    chain,
                    index,
                    device,
                })?;
                if let Some((params, name, shape)) = state {
                    editor.apply(Command::SetDeviceState {
                        device: id,
                        params,
                        name,
                        shape,
                    })?;
                }
                map_inner_copies(editor, &pairs)
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.choose(Some(id));
        Ok(DeviceAdded {
            device: id,
            track,
            new_track: false,
            default_preset: default_name,
        })
    }

    /// Inserts a copy of `device` with new IDs for it and everything in it,
    /// selected, at `place` as one undo step named `label`.
    fn insert_copy(
        &mut self,
        editor: &mut Editor,
        device: &Device,
        place: DevicePlace,
        label: &'static str,
    ) -> Result<DeviceAdded, DeviceError> {
        let mut copy = device.clone();
        // A copy goes in the chain, docked, whether or not the original
        // floats (WP-5.1, WP-2.5b), and so does everything inside a rack.
        copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
        // Spec: modulation R19. A modulator copied alone keeps its settings
        // and no mappings; one copied inside a rack maps the copies of its
        // targets inside it.
        let pairs: Vec<(&Device, DeviceId)> =
            device.walk().zip(copy.walk().map(|d| d.id)).collect();
        copy.clear_mappings();
        let id = copy.id;
        editor
            .transact(label, |editor| {
                match place.chain {
                    None => editor.apply(Command::InsertDevice {
                        track: place.track,
                        index: place.index,
                        device: copy,
                    }),
                    Some(chain) => editor.apply(Command::InsertChainDevice {
                        chain,
                        index: place.index,
                        device: copy,
                    }),
                }?;
                map_inner_copies(editor, &pairs)
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.choose(Some(id));
        Ok(DeviceAdded {
            device: id,
            track: place.track,
            new_track: false,
            default_preset: None,
        })
    }

    /// Inserts copies of `chains` with new IDs from `index` in `rack`'s list
    /// of chains, as one undo step named after Insert Chain, and selects
    /// them.
    fn insert_chain_copies(
        &mut self,
        editor: &mut Editor,
        chains: &[Chain],
        rack: DeviceId,
        index: usize,
    ) -> Result<ChainAdded, DeviceError> {
        let mut copies = chains.to_vec();
        for copy in &mut copies {
            copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
        }
        let pairs: Vec<(&Device, DeviceId)> = chains
            .iter()
            .flat_map(Chain::devices_deep)
            .zip(copies.iter().flat_map(Chain::devices_deep).map(|d| d.id))
            .collect();
        for copy in &mut copies {
            copy.devices.iter_mut().for_each(Device::clear_mappings);
        }
        let added: Vec<ChainId> = copies.iter().map(|c| c.id).collect();
        editor
            .transact("Insert Chain", |editor| {
                for (n, chain) in copies.into_iter().enumerate() {
                    editor.apply(Command::AddChain {
                        rack,
                        index: index + n,
                        chain,
                    })?;
                }
                map_inner_copies(editor, &pairs)
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.select_new_chains(&added);
        Ok(ChainAdded {
            chain: added
                .first()
                .copied()
                .ok_or_else(|| DeviceError::Unavailable("there's no chain to insert".to_owned()))?,
            rack,
            chains: added,
        })
    }

    /// Selects `chains` alone and gives their chain list the focus.
    fn select_new_chains(&mut self, chains: &[ChainId]) {
        self.chains_selected = chains.iter().copied().collect();
        self.chain_anchor = chains.last().copied();
        self.chain_list_focus = true;
    }

    /// Puts a copy of each of `chains` straight after it, as one undo step.
    fn duplicate_chains(
        &mut self,
        editor: &mut Editor,
        chains: &[ChainId],
    ) -> Result<ChainAdded, DeviceError> {
        let doc = editor.document();
        let mut placed = Vec::with_capacity(chains.len());
        for &chain in chains {
            let (rack, index) = doc
                .chain_place(chain)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
            let copy = doc.chain(chain).cloned().expect("the chain has a place");
            placed.push((rack, index, copy));
        }
        // Last first, so earlier chains' places hold while the copies go in.
        placed.sort_by_key(|(_, index, _)| std::cmp::Reverse(*index));
        let rack = placed[0].0;
        let mut added = Vec::with_capacity(placed.len());
        editor
            .transact("Insert Chain", |editor| {
                for (rack, index, original) in placed {
                    let mut copy = original.clone();
                    copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
                    let pairs: Vec<(&Device, DeviceId)> = original
                        .devices_deep()
                        .zip(copy.devices_deep().map(|d| d.id))
                        .collect();
                    copy.devices.iter_mut().for_each(Device::clear_mappings);
                    added.push(copy.id);
                    editor.apply(Command::AddChain {
                        rack,
                        index: index + 1,
                        chain: copy,
                    })?;
                    map_inner_copies(editor, &pairs)?;
                }
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        added.reverse();
        self.select_new_chains(&added);
        Ok(ChainAdded {
            chain: added[0],
            rack,
            chains: added,
        })
    }

    /// Deletes chains as one undo step.
    fn delete_chains(
        &mut self,
        editor: &mut Editor,
        chains: &[ChainId],
    ) -> Result<(), DeviceError> {
        editor
            .transact("Delete Chain", |editor| {
                chains
                    .iter()
                    .try_for_each(|&chain| editor.apply(Command::RemoveChain { chain }).map(|_| ()))
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        for chain in chains {
            self.chains_selected.remove(chain);
        }
        Ok(())
    }

    /// Solos chains as the Solo button does (racks spec, R23): exclusive,
    /// where a click on the only soloed chain unsolos it, or one by one
    /// with `add`; one undo step.
    fn solo_chains(
        &mut self,
        editor: &mut Editor,
        targets: &[ChainId],
        add: bool,
    ) -> Result<(), DeviceError> {
        let doc = editor.document();
        let (rack, _) = doc
            .chain_place(targets[0])
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {}", targets[0])))?;
        let chains = &doc.device(rack).expect("the chain has a rack").chains;
        let soloed = |c: &Chain| c.mixer.solo;
        let wanted: Vec<(ChainId, bool)> = if add {
            chains
                .iter()
                .filter(|c| targets.contains(&c.id))
                .map(|c| (c.id, !c.mixer.solo))
                .collect()
        } else {
            let only = chains.iter().filter(|c| soloed(c)).count() == targets.len()
                && chains.iter().all(|c| soloed(c) == targets.contains(&c.id));
            chains
                .iter()
                .filter(|c| soloed(c) != (targets.contains(&c.id) && !only))
                .map(|c| (c.id, !c.mixer.solo))
                .collect()
        };
        editor
            .transact("Solo chain", |editor| {
                wanted.into_iter().try_for_each(|(chain, solo)| {
                    editor
                        .apply(Command::SetChainSolo { chain, solo })
                        .map(|_| ())
                })
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))
    }

    /// Changes the chain selection as a click, Shift-click, Cmd-click, Up or
    /// Down does (racks spec, R22).
    fn select_chain(&mut self, doc: &Document, args: SelectChainArgs) -> Result<(), DeviceError> {
        let SelectChainArgs { chain, mode } = args;
        if matches!(mode, ChainSelect::Previous | ChainSelect::Next) {
            let rack = self
                .focus_rack(doc)
                .ok_or_else(|| DeviceError::Unavailable("select a rack first".to_owned()))?;
            let selected = self.rack_selection(rack);
            let last = rack.chains.len().saturating_sub(1);
            let position =
                |c: Option<&ChainId>| c.and_then(|c| rack.chains.iter().position(|x| x.id == *c));
            let at = if mode == ChainSelect::Previous {
                position(selected.first()).map(|i| i.saturating_sub(1))
            } else {
                position(selected.last()).map(|i| (i + 1).min(last))
            };
            let target = at
                .and_then(|i| rack.chains.get(i))
                .map(|c| c.id)
                .ok_or_else(|| DeviceError::Unavailable("the rack has no chains".to_owned()))?;
            let others: Vec<ChainId> = rack.chains.iter().map(|c| c.id).collect();
            self.chains_selected.retain(|c| !others.contains(c));
            self.chains_selected.insert(target);
            self.chain_anchor = Some(target);
            self.chain_list_focus = true;
            return Ok(());
        }
        let chain = chain
            .ok_or_else(|| DeviceError::InvalidArgs(format!("`chain` is needed to {mode:?}")))?;
        let rack = doc
            .chain_rack(chain)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
        let shown = self.rack_selection(rack);
        let mut picked: Vec<ChainId> = match mode {
            ChainSelect::Toggle => {
                let mut picked = shown;
                match picked.iter().position(|c| *c == chain) {
                    Some(at) => {
                        picked.remove(at);
                    }
                    None => picked.push(chain),
                }
                picked
            }
            ChainSelect::Range => {
                let position = |c: ChainId| rack.chains.iter().position(|x| x.id == c);
                let from = self
                    .chain_anchor
                    .and_then(position)
                    .or_else(|| shown.first().copied().and_then(position))
                    .unwrap_or(0);
                let to = position(chain).unwrap_or(from);
                rack.chains[from.min(to)..=from.max(to)]
                    .iter()
                    .map(|c| c.id)
                    .collect()
            }
            _ => vec![chain],
        };
        picked.dedup();
        let in_rack: Vec<ChainId> = rack.chains.iter().map(|c| c.id).collect();
        self.chains_selected.retain(|c| !in_rack.contains(c));
        self.chains_selected.extend(picked);
        // A range keeps the anchor it grew from.
        if mode != ChainSelect::Range || self.chain_anchor.is_none() {
            self.chain_anchor = Some(chain);
        }
        self.chain_list_focus = true;
        Ok(())
    }

    /// Makes the chain `device` is in the selected one of each rack above it,
    /// unfolds those racks and shows their devices views, so the device is
    /// in view (racks spec, R25). Returns whether a rack's views changed.
    fn reveal(&mut self, editor: &Editor, device: DeviceId) -> bool {
        let doc = editor.document();
        let mut path = Vec::new();
        let mut current = device;
        while let Some(chain) = doc.device_place(current).and_then(|p| p.chain) {
            let Some((rack, _)) = doc.chain_place(chain) else {
                break;
            };
            path.push((rack, chain));
            current = rack;
        }
        let mut changed = false;
        for (rack, chain) in path {
            let doc = editor.document();
            let Some(holder) = doc.device(rack) else {
                continue;
            };
            let siblings: Vec<ChainId> = holder.chains.iter().map(|c| c.id).collect();
            self.chains_selected.retain(|c| !siblings.contains(c));
            self.chains_selected.insert(chain);
            self.folded.remove(&rack);
            let views = self.views_of(rack);
            if !views.devices {
                self.rack_views
                    .insert(rack, views.with(bl_model::RackView::Devices, true));
                changed = true;
            }
        }
        changed
    }

    /// Makes a chain in a rack for the chain list's drop area
    /// (`chain.create`): empty, or holding a new device, a device made from
    /// a preset file, or a device moved from where it is.
    fn create_chain(
        &mut self,
        editor: &mut Editor,
        args: CreateChainArgs,
        user_library: Option<&Path>,
        media: &MediaStore,
        outcome: &mut Outcome,
    ) -> Result<ChainAdded, DeviceError> {
        let CreateChainArgs {
            rack,
            index,
            device,
            preset,
            move_device,
            returns,
        } = args;
        if [device.is_some(), preset.is_some(), move_device.is_some()]
            .iter()
            .filter(|given| **given)
            .count()
            > 1
        {
            return Err(DeviceError::InvalidArgs(
                "give at most one of `device`, `preset` and `move_device`".to_owned(),
            ));
        }
        let doc = editor.document();
        let holder = doc
            .device(rack)
            .filter(|d| d.rack_kind().is_some())
            .ok_or_else(|| DeviceError::InvalidArgs(format!("{rack} isn't a rack")))?;
        let drum = holder.rack_kind() == Some(RackKind::Drum);
        if returns && !drum {
            return Err(DeviceError::InvalidArgs(format!(
                "{rack} isn't a drum rack, so it has no return chains"
            )));
        }
        // A drum rack's chains go before its return chains, and a return
        // chain after them.
        let drum_end = holder.chains.iter().take_while(|c| !c.is_return).count();
        let index = index
            .unwrap_or(if returns || !drum {
                holder.chains.len()
            } else {
                drum_end
            })
            .min(holder.chains.len());
        let pad = (drum && !returns).then(|| {
            let first = self.views_of(rack).first_pad;
            (first..=127)
                .chain(0..first)
                .find(|note| {
                    !holder
                        .chains
                        .iter()
                        .any(|c| !c.is_return && c.receive == bl_model::Receive::Note(*note))
                })
                .unwrap_or(first)
        });
        let moved_title = move_device
            .map(|moved| {
                doc.device(moved)
                    .map(|d| d.title().to_owned())
                    .ok_or_else(|| DeviceError::InvalidArgs(format!("no {moved}")))
            })
            .transpose()?;
        let chain_id: ChainId = editor.new_id();
        let name = moved_title.unwrap_or_default();
        let mut made = None;
        if let Some(device) = device {
            made = Some(Ready {
                device: new_device(
                    editor.new_id(),
                    &AddDeviceArgs {
                        device,
                        track: None,
                        chain: None,
                        index: None,
                    },
                    &self.types,
                ),
                state: None,
                original: None,
                default_name: None,
            });
        } else if let Some(path) = preset {
            let path = preset_path(&path)?;
            // Spec: racks R46. A chain preset dropped on the drop area is
            // a chain of its own.
            if rack_presets::is_chain_preset(&path) {
                let args = LoadChainPresetArgs {
                    path: path.to_string_lossy().into_owned(),
                    rack: Some(rack),
                    index: Some(index),
                    chain: None,
                };
                return self.load_chain_preset(args, editor, user_library, media, outcome);
            }
            let saved = rack_presets::open_preset(&path, media, user_library, outcome)?;
            made = Some(self.ready(editor, saved, &preset::preset_name(&path))?);
        }
        let held = made.as_ref().map(|ready| ready.device.clone());
        let device_id = held.as_ref().map(|device| device.id);
        let (state, original) = made.map_or((None, None), |ready| (ready.state, ready.original));
        let pairs: Vec<(&Device, DeviceId)> = original
            .iter()
            .flat_map(Device::walk)
            .zip(held.iter().flat_map(Device::walk).map(|d| d.id))
            .collect();
        editor
            .transact("Insert Chain", |editor| {
                let mut chain = Chain::new(chain_id, name).with_devices(held.into_iter().collect());
                chain.is_return = returns;
                if let Some(note) = pad {
                    chain.receive = bl_model::Receive::Note(note);
                }
                editor.apply(Command::AddChain { rack, index, chain })?;
                if let (Some(device), Some((params, name, shape))) = (device_id, state) {
                    editor.apply(Command::SetDeviceState {
                        device,
                        params,
                        name,
                        shape,
                    })?;
                }
                if let Some(device) = move_device {
                    editor.apply(Command::MoveDeviceToChain {
                        device,
                        chain: chain_id,
                        index: 0,
                    })?;
                }
                map_inner_copies(editor, &pairs)
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.select_new_chains(&[chain_id]);
        Ok(ChainAdded {
            chain: chain_id,
            rack,
            chains: vec![chain_id],
        })
    }

    /// Puts a chain's devices in a track's own chain and deletes the chain
    /// (`chain.drop_on_track`, racks spec, R24).
    fn drop_chain_on_track(
        &mut self,
        editor: &mut Editor,
        args: &DropChainOnTrackArgs,
    ) -> Result<(), DeviceError> {
        let DropChainOnTrackArgs {
            chain,
            track,
            index,
        } = *args;
        let doc = editor.document();
        let held: Vec<(DeviceId, DeviceRole)> = doc
            .chain(chain)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?
            .devices
            .iter()
            .map(|d| (d.id, d.role))
            .collect();
        if doc.track(track).is_none() {
            return Err(DeviceError::InvalidArgs(format!("no {track}")));
        }
        let first = held
            .first()
            .map_or(DeviceRole::AudioEffect, |(_, role)| *role);
        let start = index.unwrap_or_else(|| default_index(doc, track, first, None));
        editor
            .transact("Move Chain", |editor| {
                for (n, (device, _)) in held.iter().enumerate() {
                    editor.apply(Command::MoveDevice {
                        device: *device,
                        track,
                        index: start + n,
                    })?;
                }
                editor.apply(Command::RemoveChain { chain }).map(|_| ())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))
    }

    /// Group (Cmd-G): wraps devices of one chain, or chains of one rack, in
    /// a new rack (the racks spec, R9, R10).
    fn group(&mut self, editor: &mut Editor, args: GroupArgs) -> Result<Grouped, DeviceError> {
        let doc = editor.document();
        let chains = args.chains.unwrap_or_default();
        let devices = args.devices.unwrap_or_default();
        if !chains.is_empty() {
            if !devices.is_empty() {
                return Err(DeviceError::InvalidArgs(
                    "group devices or chains, not both".to_owned(),
                ));
            }
            return self.group_chains(editor, &chains);
        }
        let devices = if devices.is_empty() {
            vec![
                self.selected
                    .ok_or_else(|| DeviceError::Unavailable("select a device first".to_owned()))?,
            ]
        } else {
            devices
        };
        let mut placed = Vec::with_capacity(devices.len());
        for &id in &devices {
            let device = doc
                .device(id)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {id}")))?;
            let place = doc
                .device_place(id)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {id}")))?;
            placed.push((place.index, device.role, device.title().to_owned()));
        }
        placed.sort();
        let kind = RackKind::holding(placed.iter().map(|(_, role, _)| *role));
        let rack: DeviceId = editor.new_id();
        let chain: ChainId = editor.new_id();
        let first = placed[0].2.clone();
        editor
            .apply(Command::GroupDevices {
                devices,
                rack: Device::rack(rack, kind).with_chain(Chain::new(chain, first)),
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.choose(Some(rack));
        Ok(Grouped { rack, chain })
    }

    /// Group with several chains of one rack selected (R10): a new chain in
    /// the first one's place holds a new rack of the same kind, and they
    /// become its chains.
    fn group_chains(
        &mut self,
        editor: &mut Editor,
        chains: &[ChainId],
    ) -> Result<Grouped, DeviceError> {
        let doc = editor.document();
        let mut placed = Vec::with_capacity(chains.len());
        for &chain in chains {
            let (rack, index) = doc
                .chain_place(chain)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
            placed.push((index, chain, rack));
        }
        placed.sort();
        let rack = placed[0].2;
        if placed.iter().any(|&(.., r)| r != rack) {
            return Err(DeviceError::Unavailable(
                "the chains to group must be in one rack".to_owned(),
            ));
        }
        let kind = doc
            .device(rack)
            .and_then(Device::rack_kind)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("{rack} isn't a rack")))?;
        if kind == RackKind::Drum {
            // Spec: drum-racks R26. A drum rack's chains group into one
            // drum chain holding an instrument rack.
            let holder = doc.device(rack).cloned().expect("the rack exists");
            return self.group_drum_chains(editor, &holder, chains);
        }
        let (inner, outer): (DeviceId, ChainId) = (editor.new_id(), editor.new_id());
        editor
            .transact("Group", |editor| {
                editor.apply(Command::AddChain {
                    rack,
                    index: placed[0].0,
                    chain: Chain::new(outer, "Chain").with_devices(vec![Device::rack(inner, kind)]),
                })?;
                for &(_, chain, _) in &placed {
                    let end = editor
                        .document()
                        .device(inner)
                        .map_or(0, |d| d.chains.len());
                    editor.apply(Command::MoveChain {
                        chain,
                        rack: inner,
                        index: end,
                    })?;
                }
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.choose(Some(inner));
        Ok(Grouped {
            rack: inner,
            chain: outer,
        })
    }

    /// Ungroup (Cmd-Shift-G): takes a rack of one chain, or none, apart
    /// (R11), selecting the first device that took its place.
    fn ungroup(&mut self, editor: &mut Editor, rack: DeviceId) -> Result<Ungrouped, DeviceError> {
        let current = editor
            .document()
            .device(rack)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {rack}")))?;
        if current.rack_kind().is_none() {
            return Err(DeviceError::Unavailable(format!("{rack} isn't a rack")));
        }
        if current.chains.len() > 1 {
            return Err(DeviceError::Unavailable(
                "a rack of several chains can't be ungrouped".to_owned(),
            ));
        }
        let devices: Vec<DeviceId> = current
            .chains
            .first()
            .map(|chain| chain.devices.iter().map(|d| d.id).collect())
            .unwrap_or_default();
        editor
            .apply(Command::UngroupDevice { device: rack })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.choose(devices.first().copied());
        Ok(Ungrouped { devices })
    }
}

/// A new device as `device.add` makes it: a rack of its kind, or a device of
/// the type and role the arguments name.
fn new_device(id: DeviceId, args: &AddDeviceArgs, types: &[DeviceType]) -> Device {
    if let DeviceKind::Rack { kind } = args.device.kind {
        return Device::rack(id, kind);
    }
    let known = types.iter().find(|t| t.kind == args.device.kind);
    let mut device = Device::new(id, args.device.name.clone(), args.device.kind.clone())
        .with_role(args.device.role);
    if args.device.sidechain || known.is_some_and(|t| t.sidechain) {
        device = device.with_sidechain();
    }
    // Spec: external-devices R9. A device whose type works with hardware
    // starts with its routing, which `device.set_external` sets.
    if known.is_some_and(|t| t.external) {
        device = device.with_external();
    }
    if args.device.midi_sidechain {
        device = device.with_midi_sidechain();
    }
    device
}

/// Selects one track in the Arrangement, which the Device View then shows.
fn select_track(
    arrangement: &mut ArrangementView,
    doc: &Document,
    track: TrackId,
) -> Result<(), DeviceError> {
    arrangement
        .run(view_ids::SELECT_TRACKS, json!({ "tracks": [track] }), doc)
        .map(|_| ())
        .map_err(|_| DeviceError::Failed(format!("couldn't select {track}")))
}

/// Loads a preset file into a device as one undo step (`device.load_preset`,
/// and hot-swapping). A described device takes its values clamped to its
/// parameters' ranges, without values for parameters it doesn't have (a
/// preset from another version of the device, say: the device framework
/// spec, R7).
fn load_preset(
    view: &DeviceView,
    editor: &mut Editor,
    device: DeviceId,
    path: &Path,
) -> Result<(), DeviceError> {
    let loaded = Preset::load(path).map_err(|e| DeviceError::Failed(e.to_string()))?;
    let current = editor
        .document()
        .device(device)
        .ok_or_else(|| DeviceError::InvalidArgs(format!("no {device}")))?;
    loaded
        .check_fits(current)
        .map_err(|e| DeviceError::Failed(e.to_string()))?;
    editor
        .apply(Command::SetDeviceState {
            device,
            params: view.sanitize(&loaded.kind, loaded.params),
            name: preset::preset_name(path),
            shape: loaded.shape,
        })
        .map(|_| ())
        .map_err(|e| DeviceError::Failed(e.to_string()))
}

fn no_library() -> DeviceError {
    DeviceError::Unavailable("there's no User Library to save presets in".to_owned())
}

/// A preset path from a command's argument: absolute, ending `.blpreset`.
fn preset_path(path: &str) -> Result<PathBuf, DeviceError> {
    let path = PathBuf::from(path);
    if path.as_os_str().is_empty() || !preset::is_preset(&path) {
        return Err(DeviceError::InvalidArgs(format!(
            "a preset file's name ends .{}",
            preset::PRESET_EXTENSION
        )));
    }
    std::path::absolute(&path).map_err(|e| DeviceError::InvalidArgs(e.to_string()))
}

fn preset_result(device: DeviceId, path: &Path) -> PresetResult {
    PresetResult {
        device,
        path: path.to_string_lossy().into_owned(),
        name: preset::preset_name(path),
    }
}

/// The folders from the User Library down to `device`'s presets, as far as
/// they exist: hot-swapping opens them in the browser.
fn presets_route(library: &Path, device: &Device) -> Vec<PathBuf> {
    let target = preset::presets_folder(library, device);
    let mut route = Vec::new();
    let mut folder = library.to_path_buf();
    if let Ok(rest) = target.strip_prefix(library) {
        for part in rest {
            folder.push(part);
            if !folder.is_dir() {
                break;
            }
            route.push(folder.clone());
        }
    }
    route
}

/// Maps the copies of the modulators in a copied rack or chain to the
/// copies of their targets inside it (the modulation spec, R19); a mapping
/// whose target is outside the copy isn't copied. `pairs` pairs each
/// original with its copy's ID.
fn map_inner_copies(
    editor: &mut Editor,
    pairs: &[(&Device, DeviceId)],
) -> Result<(), bl_model::EditError> {
    use bl_model::modulator::ModTarget;
    let copy_of = |original: DeviceId| {
        pairs
            .iter()
            .find(|(o, _)| o.id == original)
            .map(|(_, c)| *c)
    };
    for &(original, copy) in pairs {
        for mapping in &original.mappings {
            if let ModTarget::Device { device, param } = mapping.target
                && let Some(device) = copy_of(device)
            {
                editor.apply(Command::MapModulator {
                    device: copy,
                    slot: mapping.slot,
                    target: ModTarget::Device { device, param },
                })?;
            }
        }
    }
    Ok(())
}
