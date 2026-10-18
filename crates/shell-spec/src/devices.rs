//! The Device View's commands in the command registry (WP-2.4): what the
//! Device View shows, selecting and folding devices, adding them as a
//! person does, the clipboard, presets, and hot-swapping.
//!
//! The document's own device commands (`device.insert`, `device.remove`,
//! `device.move`, `device.rename`, `device.set_enabled`,
//! `device.set_show_preset_name`, `device.set_state`, `device.set_param`,
//! `device.set_floating`, `device.compare_switch`, `device.compare_copy`)
//! are `bl-model`'s. Those here run on the shell, which knows the selection,
//! the clipboard, the User Library, and the browser. A command whose
//! `device` is left out acts on the device selected in the Device View, as
//! its key and its context-menu item do. The behaviour they follow is in the
//! device chains spec.

use bl_commands::Context::{Browser, DeviceView, Global, RackChains};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::device_layout::BODY_HEIGHT;
use bl_model::{
    ChainId, ContentHash, DeviceDescription, DeviceDisplay, DeviceId, DeviceKind, DeviceLayout,
    DeviceRole, ParamDescriptor, ParamId, RackKind, RackView, RackViews, Tap, TrackId, TrackKind,
    WindowFrame,
};
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
/// `device.expand`: show or hide a device's expanded part (WP-5.42).
pub const EXPAND: &str = "device.expand";
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
/// `device.load_sample`: load an audio file into a device that plays one
/// sample (WP-2.16).
pub const LOAD_SAMPLE: &str = "device.load_sample";
/// `device.swap_to_snapshot`: replace a Strike drum sampler with the Snapshot
/// sampler, keeping its sample's start and length (one-shot drum sampler
/// R22).
pub const SWAP_TO_SNAPSHOT: &str = "device.swap_to_snapshot";
/// `device.select_slot`: select a sample slot of a device that has them
/// (WP-5.17).
pub const SELECT_SLOT: &str = "device.select_slot";
/// `device.hot_swap`: start or end hot-swapping a device's presets.
pub const HOT_SWAP: &str = "device.hot_swap";
/// `device.parameters`: a device's parameters, described, with their values.
pub const PARAMETERS: &str = "device.parameters";
/// `device.show_sidechain`: show or hide a device's sidechain section
/// (WP-2.19b).
pub const SHOW_SIDECHAIN: &str = "device.show_sidechain";
/// `device.display`: what a device's display shows now (WP-5.43).
pub const DISPLAY: &str = "device.display";
/// `device.set_display_view`: choose the view a device's display shows
/// (WP-5.43).
pub const SET_DISPLAY_VIEW: &str = "device.set_display_view";

/// `modulator.arm_map`: arm or disarm a modulator slot's Map button
/// (WP-5.10).
pub const ARM_MAP: &str = "modulator.arm_map";
/// `device.group`: wrap the selected devices, or chains, in a new rack
/// (WP-5.1).
pub const GROUP: &str = "device.group";
/// `device.ungroup`: take the selected rack apart (WP-5.1).
pub const UNGROUP: &str = "device.ungroup";
/// `chain.copy`: copy a chain to the chain clipboard (WP-5.1).
pub const CHAIN_COPY: &str = "chain.copy";
/// `chain.cut`: cut a chain to the chain clipboard.
pub const CHAIN_CUT: &str = "chain.cut";
/// `chain.paste`: paste the clipboard's chain into a rack.
pub const CHAIN_PASTE: &str = "chain.paste";
/// `chain.duplicate`: duplicate a chain straight after itself.
pub const CHAIN_DUPLICATE: &str = "chain.duplicate";
/// `chain.delete`: delete the selected chains (WP-5.2).
pub const CHAIN_DELETE: &str = "chain.delete";
/// `chain.select`: select a rack's chains in its chain list.
pub const CHAIN_SELECT: &str = "chain.select";
/// `chain.solo`: solo chains as the Solo button does.
pub const CHAIN_SOLO: &str = "chain.solo";
/// `chain.hot_swap`: arm a chain for hot-swap.
pub const CHAIN_HOT_SWAP: &str = "chain.hot_swap";
/// `chain.create`: make a chain at the end of a rack's list holding a
/// device, as the chain list's drop area does.
pub const CHAIN_CREATE: &str = "chain.create";
/// `chain.drop_on_track`: put a chain's devices in a track's own chain.
pub const CHAIN_DROP_ON_TRACK: &str = "chain.drop_on_track";
/// `rack.set_view`: show or hide a view of a rack (WP-5.2).
pub const RACK_SET_VIEW: &str = "rack.set_view";

/// `device.windows`: the floating devices' windows (WP-2.5b).
pub const WINDOWS: &str = "device.windows";
/// `device.set_window`: where a floating device's window sits.
pub const SET_WINDOW: &str = "device.set_window";
/// `device.set_floating`: the document's command that floats or docks a
/// device (`bl-model`'s).
pub const SET_FLOATING: &str = "device.set_floating";

/// `device.compare_switch`: make a native device's other A/B state active
/// (`bl-model`'s).
pub const COMPARE_SWITCH: &str = "device.compare_switch";
/// `device.compare_copy`: copy a native device's active A/B state over the
/// other (`bl-model`'s).
pub const COMPARE_COPY: &str = "device.compare_copy";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    STATE,
    ADD,
    SELECT,
    FOLD,
    EXPAND,
    CUT,
    COPY,
    PASTE,
    DUPLICATE,
    SAVE_PRESET,
    SAVE_DEFAULT_PRESET,
    LOAD_PRESET,
    LOAD_SAMPLE,
    SWAP_TO_SNAPSHOT,
    SELECT_SLOT,
    HOT_SWAP,
    PARAMETERS,
    SHOW_SIDECHAIN,
    DISPLAY,
    SET_DISPLAY_VIEW,
    ARM_MAP,
    WINDOWS,
    SET_WINDOW,
    GROUP,
    UNGROUP,
    CHAIN_COPY,
    CHAIN_CUT,
    CHAIN_PASTE,
    CHAIN_DUPLICATE,
    CHAIN_DELETE,
    CHAIN_SELECT,
    CHAIN_SOLO,
    CHAIN_HOT_SWAP,
    CHAIN_CREATE,
    CHAIN_DROP_ON_TRACK,
    RACK_SET_VIEW,
];

/// The document's device commands that act on the selected device when
/// `device` is left out, as their keys and menu items do.
pub const ON_SELECTED: &[&str] = &[
    "device.remove",
    "device.rename",
    "device.set_enabled",
    "device.set_show_preset_name",
    SET_FLOATING,
    COMPARE_SWITCH,
    COMPARE_COPY,
];

/// Whether `id` is one of the Device View's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id) || crate::pads::handles(id)
}

/// A device frame's title bar height at 100%, in points (observed in Live
/// 12.4.6; layout L-DEV-004): also a folded device's width.
pub const TITLE_BAR_HEIGHT: f32 = 16.0;

/// The gap between devices in a chain at 100%, in points (observed).
pub const DEVICE_GAP: f32 = 7.0;

/// The width at 100%, in points, of an unfolded device whose type doesn't
/// describe its layout (the test devices of WP-2.4): a described device is
/// its layout's docked width ([`frame_width`]).
pub const DEFAULT_DEVICE_WIDTH: f32 = 120.0;

/// The width of a device's sidechain strip at 100%, in points: the strip
/// down its left edge with the section's show/hide toggle and Sidechain
/// Listen (observed; its width is to observe, and Beatlayer uses a title
/// bar's height).
pub const SIDECHAIN_STRIP_WIDTH: f32 = TITLE_BAR_HEIGHT;

/// The width of a shown sidechain section at 100%, in points (to observe).
pub const SIDECHAIN_SECTION_WIDTH: f32 = 80.0;

/// The width at 100% of a rack's view column, down its left edge under the
/// title bar (racks spec, R18): a title bar's height, as a folded device is
/// wide.
pub const RACK_COLUMN_WIDTH: f32 = TITLE_BAR_HEIGHT;

/// The width at 100% of the place the Macro Controls take in a rack while
/// that view is shown (WP-5.4 fills it; to observe).
pub const RACK_MACROS_WIDTH: f32 = 100.0;

/// The width at 100% of a rack's chain list (to observe).
pub const RACK_CHAIN_LIST_WIDTH: f32 = 168.0;

/// The width at 100% of the end bracket that closes a rack's devices
/// (racks spec, R18; to observe).
pub const RACK_END_BRACKET_WIDTH: f32 = 8.0;

/// The width at 100% of a rack's devices view while the chain holds no
/// device, or none is shown (to observe).
pub const RACK_EMPTY_DEVICES_WIDTH: f32 = 96.0;

/// The height at 100% of a row of the chain list: the chain's name,
/// activator, Solo and Hot-Swap on the first line, and Volume and Pan on
/// the second (to observe).
pub const CHAIN_ROW_HEIGHT: f32 = 30.0;

/// The height at 100% of the chain list's drop area, below the rows.
pub const CHAIN_DROP_AREA_HEIGHT: f32 = 26.0;

/// The width at 100% of a rack's devices view: its devices left to right
/// with the gap before each, and its end bracket (racks spec, R18).
#[must_use]
pub fn rack_devices_width(rack: &RackFrame) -> f32 {
    let inside = if rack.devices.is_empty() {
        RACK_EMPTY_DEVICES_WIDTH
    } else {
        rack.devices
            .iter()
            .fold(DEVICE_GAP, |w, d| w + frame_width(d) + DEVICE_GAP)
    };
    inside + RACK_END_BRACKET_WIDTH
}

/// The width at 100% of a rack's chain list: a drum rack's grows with the
/// switches that show its Input/Output columns and its sends (drum racks
/// spec, R21).
#[must_use]
pub fn rack_chain_list_width(rack: &RackFrame) -> f32 {
    let returns = rack.chains.iter().filter(|c| c.output.is_some()).count();
    RACK_CHAIN_LIST_WIDTH
        + if rack.pads.is_some() && rack.views.input_output {
            crate::pads::DRUM_IO_WIDTH
        } else {
            0.0
        }
        + if rack.pads.is_some() && rack.views.send {
            returns as f32 * crate::pads::DRUM_SEND_WIDTH
        } else {
            0.0
        }
}

/// The width at 100% of a drum rack's pad view, while it shows; 0 for any
/// other rack.
#[must_use]
pub fn rack_pads_width(rack: &RackFrame) -> f32 {
    if rack.pads.is_some() && rack.views.pads {
        crate::pads::RACK_PAD_WIDTH
    } else {
        0.0
    }
}

/// How wide an unfolded rack is at 100%: its view column and each view it
/// shows, left to right (racks spec, R18).
#[must_use]
pub fn rack_width(rack: &RackFrame) -> f32 {
    RACK_COLUMN_WIDTH
        + if rack.views.macros {
            RACK_MACROS_WIDTH
        } else {
            0.0
        }
        + rack_pads_width(rack)
        + if rack.views.chain_list {
            rack_chain_list_width(rack)
        } else {
            0.0
        }
        + if rack.views.devices {
            rack_devices_width(rack)
        } else {
            0.0
        }
}

/// The width at 100%, in points, of a device's own body: its layout's
/// docked width when its type describes one (WP-2.5), and
/// [`DEFAULT_DEVICE_WIDTH`] otherwise.
#[must_use]
pub fn body_width(frame: &DeviceFrame) -> f32 {
    if let Some(rack) = &frame.rack {
        return rack_width(rack);
    }
    frame.panel.as_ref().map_or(DEFAULT_DEVICE_WIDTH, |panel| {
        panel.layout(frame.expanded).docked_size().width
    })
}

/// The width at 100% of what sits left of a device's body: its sidechain
/// strip and, while shown, its sidechain section (WP-2.19b); 0 without a
/// sidechain input.
#[must_use]
pub fn sidechain_width(frame: &DeviceFrame) -> f32 {
    frame.sidechain.as_ref().map_or(0.0, |s| {
        SIDECHAIN_STRIP_WIDTH
            + if s.shown {
                SIDECHAIN_SECTION_WIDTH
            } else {
                0.0
            }
    })
}

/// How wide a device's frame is at 100%: a title bar wide when folded or
/// floating (a floating device's place in the chain is a strip, WP-2.5b);
/// otherwise its body's width ([`body_width`]), with its sidechain strip
/// and, while shown, its sidechain section to the left (WP-2.19b).
#[must_use]
pub fn frame_width(frame: &DeviceFrame) -> f32 {
    if frame.folded || frame.floating || frame.rack.as_ref().is_some_and(RackFrame::is_strip) {
        return TITLE_BAR_HEIGHT;
    }
    docked_width(frame)
}

/// How wide a device is at 100% unfolded and docked: its body's width with
/// its sidechain strip and, while shown, its sidechain section.
#[must_use]
pub fn docked_width(frame: &DeviceFrame) -> f32 {
    body_width(frame) + sidechain_width(frame)
}

/// How tall a device's body is at 100% at its docked size: its layout's
/// docked height, or a device body's 158 points without one.
#[must_use]
pub fn body_height(frame: &DeviceFrame) -> f32 {
    frame.panel.as_ref().map_or(BODY_HEIGHT, |panel| {
        panel.layout(frame.expanded).docked_size().height
    })
}

/// The smallest a floating device's window shows at 100%, in points: the
/// device at its docked size under its title bar (IMP-14: the docked size
/// is the minimum).
#[must_use]
pub fn window_min_size(frame: &DeviceFrame) -> (f32, f32) {
    (docked_width(frame), TITLE_BAR_HEIGHT + body_height(frame))
}

/// Whether a floating device's window grows across and down: only if its
/// layout has a display cell that does (IMP-14). A device that doesn't
/// grow floats at its docked size.
#[must_use]
pub fn window_grows(frame: &DeviceFrame) -> (bool, bool) {
    frame
        .panel
        .as_ref()
        .map_or((false, false), |panel| panel.layout(frame.expanded).grows())
}

/// What the generic UI needs to draw a device (WP-2.5): its type's
/// description and its parameters' values now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DevicePanel {
    /// Its parameters' descriptors and its layout.
    pub description: DeviceDescription,
    /// Each parameter's value now, in the description's order, in the
    /// parameter's own units.
    pub values: Vec<f64>,
    /// Whether this is a plug-in device's panel (WP-2.10, D19): eight knobs
    /// mapped to the plug-in's own parameters, with the rest of the body a
    /// target that opens the plug-in's editor. False for a native device.
    #[serde(default)]
    pub plugin: bool,
    /// For a device with sample slots (WP-5.17): the slot whose controls
    /// the panel shows, a view setting that isn't saved with the set.
    #[serde(default)]
    pub selected_slot: u32,
    /// For a device with sample slots: whether each slot holds a sample.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loaded_slots: Vec<bool>,
    /// For a device with sample slots: the key each slot plays, as Live
    /// names notes (`C3`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slot_keys: Vec<String>,
}

impl DevicePanel {
    /// The layout as drawn: with the expanded part while `expanded`
    /// ([`DeviceLayout::shown`]).
    #[must_use]
    pub fn layout(&self, expanded: bool) -> DeviceLayout {
        self.description.layout.shown(expanded)
    }

    /// The value of the parameter at `index` in the description.
    #[must_use]
    pub fn value(&self, index: usize) -> f64 {
        self.values.get(index).copied().unwrap_or_else(|| {
            self.description
                .params
                .get(index)
                .map_or(0.0, |p| p.default)
        })
    }
}

impl DeviceFrame {
    /// The device's own items in its Show Options menu, in the order its
    /// type lists them: they follow Save as Default Preset (C-DEVICE-015).
    #[must_use]
    pub fn menu_options(&self) -> Vec<MenuOption<'_>> {
        let Some(panel) = &self.panel else {
            return Vec::new();
        };
        let params = &panel.description.params;
        panel
            .description
            .menu_options
            .iter()
            .filter_map(|id| params.iter().position(|p| p.id == *id))
            .map(|index| MenuOption {
                param: params[index].id,
                name: &params[index].name,
                on: panel.value(index) >= 0.5,
            })
            .collect()
    }
}

/// One of a device's parameters as `device.parameters` reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ParameterState {
    /// What it is: its number, name, kind and range, default, and whether
    /// it can be automated.
    #[serde(flatten)]
    pub descriptor: ParamDescriptor,
    /// Its value now, in its own units: what `device.set_param` takes.
    pub value: f64,
    /// How the value reads, as its control shows it, such as `-6.0 dB`.
    pub text: String,
    /// Whether it's one of the device's own items in its Show Options menu
    /// rather than a control in its body (C-DEVICE-015).
    #[serde(default)]
    pub menu_option: bool,
    /// Its value in the A and B state that isn't active (device compare
    /// R11); left out for a device without two states.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<f64>,
}

/// What `device.parameters` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceParameters {
    /// The device.
    pub device: DeviceId,
    /// Which of its A and B states is active, `A` or `B` (device compare
    /// R11); left out for a device without two states.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// Its parameters, in the order its type lists them; empty if its type
    /// doesn't describe them.
    pub params: Vec<ParameterState>,
}

/// One of a device's own items in its Show Options menu: a switch its type
/// marks as a menu option (C-DEVICE-015).
#[derive(Clone, Debug, PartialEq)]
pub struct MenuOption<'a> {
    /// The parameter choosing the item turns over (`device.set_param`).
    pub param: ParamId,
    /// The item's label: the parameter's name.
    pub name: &'a str,
    /// Whether it's ticked: the switch is on.
    pub on: bool,
}

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
    /// Whether its type plays one sample, so a sample dropped on it loads
    /// into it (`device.load_sample`, WP-2.16).
    #[serde(default)]
    pub takes_sample: bool,
    /// Whether its type is the drum sampler, whose context menu offers the
    /// swap to Snapshot and Save as Default Pad (`device.swap_to_snapshot`,
    /// one-shot drum sampler R23).
    #[serde(default)]
    pub swaps_to_snapshot: bool,
    /// Whether its title shows its preset's or given name.
    pub show_preset_name: bool,
    /// Whether it's folded to its title bar.
    pub folded: bool,
    /// Whether it has an expanded part, which its title bar's expand
    /// toggle shows and hides (L-DEV-004; WP-5.42).
    #[serde(default)]
    pub expandable: bool,
    /// Whether its expanded part is shown. Never while it has none.
    #[serde(default)]
    pub expanded: bool,
    /// Whether it floats in a window of its own, its place in the chain a
    /// strip (WP-2.5b).
    #[serde(default)]
    pub floating: bool,
    /// Whether its B state is the active one, which the title bar marks
    /// "(B)" (device compare R5).
    #[serde(default)]
    pub b_active: bool,
    /// Whether it's the selected device.
    pub selected: bool,
    /// Whether it's the device being hot-swapped.
    pub hot_swap: bool,
    /// A multisampler's views and selected layers (WP-5.13, R30); left out
    /// for any other device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multisampler: Option<crate::multisampler::MultisamplerState>,
    /// Its parameters and layout, for the generic UI; left out for a
    /// device whose type doesn't describe them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<DevicePanel>,
    /// Its sidechain section, if it has a sidechain input (WP-2.19b).
    pub sidechain: Option<SidechainSection>,
    /// The view its display shows, if its display has views (WP-5.43): one
    /// of its description's `views`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_view: Option<String>,
    /// For a modulator, what it maps and which Map button is armed
    /// (WP-5.10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modulator: Option<ModulatorFrame>,
    /// A rack's frame: its views, its chain list and the selected chain's
    /// devices (WP-5.2). None for a device that isn't a rack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<RackFrame>,
}

/// A rack as the Device View shows it (WP-5.2; the racks spec, R18 to R23):
/// a view column of selectors, then each view that's shown, left to right.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RackFrame {
    /// Its kind.
    pub kind: RackKind,
    /// Which views it shows: kept with the set, so reopening it shows them.
    pub views: RackViews,
    /// Its chains, in order.
    pub chains: Vec<ChainRow>,
    /// The selected chains: those clicked, or the first chain while none is.
    pub selected_chains: Vec<ChainId>,
    /// The devices of the selected chain, left to right: empty while no
    /// chain or several are selected.
    pub devices: Vec<DeviceFrame>,
    /// What the devices view says instead of devices while several chains are
    /// selected, such as `3 chains selected`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Whether the chain list has the focus, so S solos the selected chain.
    pub chain_list_focused: bool,
    /// A drum rack's pad view (drum racks spec, R11); left out for any
    /// other rack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pads: Option<crate::pads::PadFrame>,
    /// A drum rack's return tracks, which a return chain's Audio To can
    /// choose, in order (R8).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub return_tracks: Vec<crate::pads::ReturnChoice>,
    /// Whether a drum rack can take another return chain: it holds fewer
    /// than six (R7).
    #[serde(default)]
    pub can_add_return: bool,
}

impl RackFrame {
    /// Whether the rack is drawn as a strip a title bar wide: every view is
    /// hidden, so it folds into its view column (racks spec, R19).
    #[must_use]
    pub fn is_strip(&self) -> bool {
        self.views.all_hidden() && !(self.pads.is_some() && self.views.pads)
    }
}

/// One row of a rack's chain list (racks spec, R21).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChainRow {
    /// The chain.
    pub id: ChainId,
    /// Its name.
    pub name: String,
    /// Its colour, a swatch of the palette of 5 rows of 14, if it has one.
    pub colour: Option<u8>,
    /// Its Chain Activator.
    pub enabled: bool,
    /// Whether it's soloed.
    pub solo: bool,
    /// Whether it's armed for hot-swap by its Hot-Swap button.
    pub hot_swap: bool,
    /// Its Volume as linear gain; 1 is 0 dB.
    pub volume: f64,
    /// Its Pan, from -1 to 1.
    pub pan: f64,
    /// Whether it has Volume and Pan: all but a MIDI effect rack's chains do.
    pub has_mixer: bool,
    /// Whether it's selected.
    pub selected: bool,
    /// A drum chain's notes, choke group and sends (drum racks spec, R21);
    /// left out for any other chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drum: Option<crate::pads::DrumRow>,
    /// A drum rack's return chain's output (R8); left out for any other
    /// chain. A return chain has no sends, notes or choke group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<crate::pads::ReturnRow>,
}

/// One entry of the Device View selector's device list (racks spec, R25).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceListEntry {
    /// How deep it sits: a track's devices are 0, a rack's chains 1 and
    /// those chains' devices 2.
    pub depth: usize,
    /// What the list shows: a device's title, or a chain's name.
    pub label: String,
    /// The device, for a device's entry.
    pub device: Option<DeviceId>,
    /// The chain, for a chain's entry.
    pub chain: Option<ChainId>,
}

/// What a modulator's panel shows beside its parameters (WP-5.10; the
/// modulation spec, R15, R19, R23).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModulatorFrame {
    /// Its mappings, by slot.
    pub mappings: Vec<MappingRow>,
    /// The slot whose Map button is lit, while a click on a parameter maps
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armed: Option<u8>,
    /// A Curve Mod's breakpoint shape (the default one until it is edited);
    /// left out for a modulator that has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<Vec<bl_model::modulator::ShapePoint>>,
}

/// One slot's mapping as its panel names it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MappingRow {
    /// The slot, from 0 to 7.
    pub slot: u8,
    /// The parameter it maps.
    pub target: bl_model::modulator::ModTarget,
    /// The target's device and parameter, or track and control, by name,
    /// as the slot shows them.
    pub label: String,
}

impl DeviceFrame {
    /// What its title bar reads, with "(B)" after the name while the B
    /// state is active (device compare R5).
    #[must_use]
    pub fn title_text(&self) -> String {
        if self.b_active {
            format!("{} (B)", self.title)
        } else {
            self.title.clone()
        }
    }
}

/// A device's sidechain section, as its left strip and the section show it
/// (WP-2.19b; observed in Live 12.4.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SidechainSection {
    /// Whether the section is shown, by the toggle at the top of the
    /// device's left strip. It changes only the view.
    pub shown: bool,
    /// External: whether the detector listens to the sidechain.
    pub enabled: bool,
    /// What the Source chooser shows: a track's label (`3-Audio`,
    /// `A-Reverb`, `Main`) or `No Input`.
    pub source: String,
    /// Its channel: where the source track is tapped, or `null` with No
    /// Input.
    pub tap: Option<Tap>,
    /// Mix, from 0 to 1 (100 %).
    pub mix: f64,
    /// SC Gain, linear (1 is 0.00 dB).
    pub gain: f64,
    /// Sidechain Listen.
    pub listen: bool,
}

/// A floating device's window, as `device.windows` reports it (WP-2.5b).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FloatingWindow {
    /// The device.
    pub device: DeviceId,
    /// Its track.
    pub track: TrackId,
    /// The window's title: the device's title, then its track's label, as
    /// Live titles a plug-in's window (`Utility/3-Audio`; observed in Live
    /// 12.4.6 with a plug-in, `AUDelay/4-Audio`).
    pub title: String,
    /// Where the window sits and how big it is, once it has been placed;
    /// `null` until then, and the app places it.
    pub frame: Option<WindowFrame>,
    /// The smallest size its contents take at 100%, in points: the device
    /// at its docked size under its title bar.
    pub min_width: f32,
    /// See `min_width`.
    pub min_height: f32,
    /// Whether the window can grow across: only a device with a display
    /// cell that grows can.
    pub grows_across: bool,
    /// Whether the window can grow down.
    pub grows_down: bool,
    /// The device, as its window draws it.
    pub view: DeviceFrame,
    /// Whether the window shows: always, unless Auto-Hide Plug-In Windows
    /// is on (as it is in a new install) and another track is selected,
    /// when it hides until its track is selected again. A hidden window's
    /// device still floats.
    pub shown: bool,
}

/// What `device.windows` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FloatingWindows {
    /// Every floating device's window, on any track, in the order of the
    /// tracks and their chains.
    pub windows: Vec<FloatingWindow>,
}

/// Arguments of `device.set_window`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetWindowArgs {
    /// The floating device.
    pub device: DeviceId,
    /// Where its window sits and how big its contents are, in points.
    pub frame: WindowFrame,
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
    /// Whether the chain clipboard holds chains to paste.
    #[serde(default)]
    pub can_paste_chain: bool,
    /// Every device on the track as a hierarchy, each rack's devices under
    /// it chain by chain: the Device View selector's list (racks spec, R25).
    #[serde(default)]
    pub device_list: Vec<DeviceListEntry>,
    /// The Map button that is lit, on any track: a click on any parameter's
    /// control maps it to the slot (WP-5.10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armed_map: Option<ArmedMap>,
}

/// A lit Map button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArmedMap {
    /// The modulator.
    pub device: DeviceId,
    /// Its slot.
    pub slot: u8,
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

/// Arguments of `device.expand`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpandDeviceArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// Show (true) or hide (false) the expanded part; switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
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
    /// Whether it has a sidechain input, as its type declares (WP-2.19b).
    #[serde(default, skip_serializing_if = "is_false")]
    pub sidechain: bool,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
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
    /// A rack's chain to add it to, at any depth, in place of the track's
    /// own chain (WP-5.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainId>,
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
    /// A rack's chain to paste into, at any depth, in place of the track's
    /// own chain (WP-5.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainId>,
    /// Paste before this device, which may be in a rack's chain; before the
    /// selected device, or at the end of the chain with none selected, if
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<DeviceId>,
}

/// Arguments of `device.group`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupArgs {
    /// The devices to wrap in a new rack, all in one chain; the selected
    /// device if both lists are left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<Vec<DeviceId>>,
    /// Several chains of one rack to group instead: they become the chains
    /// of a new rack of the same kind, in a new chain in the first one's
    /// place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chains: Option<Vec<ChainId>>,
}

/// What `device.group` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Grouped {
    /// The new rack, now selected.
    pub rack: DeviceId,
    /// The chain that holds the devices, or, when chains were grouped, the
    /// new chain that holds the new rack.
    pub chain: ChainId,
}

/// What `device.ungroup` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Ungrouped {
    /// The devices that took the rack's place, in order; the first is now
    /// selected.
    pub devices: Vec<DeviceId>,
}

/// Arguments of `chain.copy`, `chain.cut` and `chain.duplicate`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChainArgs {
    /// The chain; the selected chains if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainId>,
}

/// Arguments of `chain.delete`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteChainsArgs {
    /// The chains; the selected ones if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chains: Option<Vec<ChainId>>,
}

/// How `chain.select` changes the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChainSelect {
    /// Select only `chain`: a click.
    #[default]
    Replace,
    /// Add `chain` to the selection, or remove it: Cmd-click (Ctrl-click).
    Toggle,
    /// Select every chain from the selected one to `chain`: Shift-click.
    Range,
    /// Select only the chain above the first selected one: Up.
    Previous,
    /// Select only the chain below the last selected one: Down.
    Next,
}

/// Arguments of `chain.select`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectChainArgs {
    /// The chain to click; left out for `previous` and `next`, which step
    /// from the chains selected now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainId>,
    /// How the selection changes; a click if left out.
    #[serde(default)]
    pub mode: ChainSelect,
}

/// Arguments of `chain.solo`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SoloChainArgs {
    /// The chain; the selected chains if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainId>,
    /// Add or remove the chain without touching the others' solo, as
    /// Cmd-click (Ctrl-click) does; otherwise it's the only one soloed.
    #[serde(default)]
    pub add: bool,
}

/// Arguments of `chain.hot_swap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HotSwapChainArgs {
    /// The chain.
    pub chain: ChainId,
    /// Arm (true) or disarm (false); switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Arguments of `chain.create`: at most one of `device`, `preset` and
/// `move_device`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateChainArgs {
    /// The rack.
    pub rack: DeviceId,
    /// Where the chain goes in the rack's list; the end if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    /// A new device to put in it, as `device.add` describes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<NewDevice>,
    /// A preset file: the chain holds a new device with its values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// A device to move into it from where it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_device: Option<DeviceId>,
    /// Make a return chain of a drum rack (the drop area of its return
    /// section): it goes after the drum chains and the rack's other return
    /// chains, holds audio effects only, and sends to the rack's own
    /// output. A drum rack holds six at most.
    #[serde(default, skip_serializing_if = "is_false")]
    pub returns: bool,
}

/// Arguments of `chain.drop_on_track`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DropChainOnTrackArgs {
    /// The chain whose devices move.
    pub chain: ChainId,
    /// The track whose own chain takes them.
    pub track: TrackId,
    /// Where the first goes in the track's chain; after the track's chain's
    /// devices of the same place in Live's order if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// Arguments of `rack.set_view`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRackViewArgs {
    /// The rack; the selected device if left out, which must be a rack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<DeviceId>,
    /// The view.
    pub view: RackView,
    /// Show (true) or hide (false); switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<bool>,
}

/// Arguments of `chain.paste`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasteChainArgs {
    /// The rack to paste into; the rack in focus if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<DeviceId>,
    /// Where it goes in the rack's list of chains; the end if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// What the chain commands that add a chain return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChainAdded {
    /// The new chain, or the first of them.
    pub chain: ChainId,
    /// The rack it is in.
    pub rack: DeviceId,
    /// Every new chain, in order.
    #[serde(default)]
    pub chains: Vec<ChainId>,
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

/// Arguments of `device.load_sample`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadSampleArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The audio file to load into the device.
    pub path: String,
    /// The sample slot to load it into, counted from 0; the slot the
    /// panel has selected for a device with several, and 0 for one with
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u8>,
}

/// Arguments of `device.select_slot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectSlotArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The slot, counted from 0.
    pub slot: u32,
}

/// What `device.load_sample` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoadSampleResult {
    /// The device that took the sample.
    pub device: DeviceId,
    /// The sample's media, by content hash: what the device and the set
    /// name it by.
    pub media: ContentHash,
    /// The slot that took it, counted from 0.
    #[serde(default)]
    pub slot: u8,
    /// The file's name, for messages.
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

/// Arguments of `device.show_sidechain`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowSidechainArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// Show (true) or hide (false); switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<bool>,
}

/// Arguments of `modulator.arm_map`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArmMapArgs {
    /// The modulator; the selected device if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The slot, from 0 to 7.
    pub slot: u8,
    /// Arm (true) or disarm (false); switches if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armed: Option<bool>,
}

/// Arguments of `device.set_display_view`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetDisplayViewArgs {
    /// The device; the selected one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The view, one of the device's description's `views`.
    pub view: String,
}

/// Adds the Device View's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs().into_iter().chain(crate::pads::specs()) {
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
                 it's on, folded, selected, or being hot-swapped, and whether it has an expanded \
                 part (`expandable`) and shows it (`expanded`); the selected device; \
                 hot-swapping; and whether there's a device to paste. A native device also \
                 lists its parameters' descriptors, its layout, and its values (`panel`); \
                 `device.parameters` reports any device's. Changes nothing.",
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
                 Device and Track). With `chain` it goes into that rack chain instead. A chain \
                 holds one instrument; a rack holds more.",
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
        CommandSpec::new(EXPAND, "Expand Device")
            .description(
                "Shows or hides a device's expanded part, as the toggle in its title bar does: \
                 the extra controls (a modulation section, say) some devices keep out of sight \
                 until asked; switches unless `expanded` says which. Only devices with an \
                 expanded part (`expandable` in `device_view.state`) have one. Like folding, it \
                 changes only the view: it isn't an undo step, and isn't saved with the set. \
                 The device's width in the chain follows it.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<ExpandDeviceArgs>()
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
                 instrument, and instruments and MIDI effects only go on MIDI tracks. With \
                 `chain` it pastes into that rack chain, or before a device that is in one.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .binding("S-EDIT-003")
            .args::<PasteDeviceArgs>()
            .result::<DeviceAdded>(),
        CommandSpec::new(DUPLICATE, "Duplicate Device")
            .description(
                "Puts a copy of a device, with its settings, straight after it, and selects the \
                 copy: one undo step, named Duplicate Device. A chain holds one instrument, so \
                 an instrument can't be duplicated; a rack is copied whole, with new IDs for \
                 its chains and devices.",
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
        CommandSpec::new(LOAD_SAMPLE, "Load Sample")
            .description(
                "Loads an audio file into a device that plays samples (the sampler, or the \
                 eight-slot drum sampler), as dropping the file on the device does: the file \
                 goes into the set's media store, named by its content hash, and the device \
                 keeps that reference, so the set reopens playing it. `slot` says which of a \
                 device's sample slots takes it (counted from 0); left out, it is the slot \
                 selected in the panel, or the only one. One undo step, named Load Sample. \
                 Reads a file outside the project. A device whose type doesn't play a sample, \
                 or a slot it doesn't have, is refused. `device.set_sample` with a null `media` \
                 clears a slot.",
            )
            .permission(Permission::External)
            .context(DeviceView)
            .args::<LoadSampleArgs>()
            .result::<LoadSampleResult>(),
        CommandSpec::new(SWAP_TO_SNAPSHOT, "Swap to Snapshot")
            .description(
                "Replaces a Strike drum sampler with the Snapshot sampler in One-Shot mode, as \
                 Strike's context menu does: the new device takes the old one's place, sample, \
                 and on or off state, and keeps its sample's Start and Length; every other \
                 setting is Snapshot's default. One undo step, named Swap to Snapshot, and the \
                 new device is selected. A device that isn't a Strike drum sampler is refused.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<DeviceArgs>()
            .result::<DeviceAdded>(),
        CommandSpec::new(SELECT_SLOT, "Select Sample Slot")
            .description(
                "Selects one of a device's sample slots, as clicking its cell in the panel \
                 does: the panel then shows that slot's controls, and a sample loaded without \
                 naming a slot goes into it. Only a device with sample slots has any (the \
                 eight-slot drum sampler has eight, counted from 0). It changes only the \
                 view: it isn't an undo step, and isn't saved with the set. The selected slot \
                 is `selected_slot` in the device's panel (`device.state`).",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<SelectSlotArgs>()
            .result::<DeviceViewState>(),
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
        CommandSpec::new(PARAMETERS, "Device Parameters")
            .description(
                "Reports a device's parameters, in the order its type lists them: each one's \
                 number (what `device.set_param` takes), name, what it does, its kind (a \
                 continuous value with its range, unit, and taper; a gain, whose value is a \
                 linear factor from 0, which is -inf dB, up to its top; a switch; a choice with \
                 its names; or a whole number), its default, whether it can be automated (every \
                 parameter can), its value now, and how that value reads. Empty for a device \
                 whose type doesn't describe its parameters, such as a plug-in that couldn't \
                 be loaded. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<DeviceArgs>()
            .result::<DeviceParameters>(),
        CommandSpec::new(SHOW_SIDECHAIN, "Show Sidechain")
            .description(
                "Shows or hides a device's sidechain section, as the toggle at the top of the \
                 device's left strip does; switches unless `shown` says which. Only devices \
                 with a sidechain input have one. It changes only the view: it isn't an undo \
                 step. The section's settings are the set's: `device.set_sidechain_input`, \
                 `device.set_sidechain_enabled`, `device.set_sidechain_mix`, \
                 `device.set_sidechain_gain`, and `device.set_sidechain_listen`.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<ShowSidechainArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(GROUP, "Group")
            .description(
                "Wraps the selected devices (or `devices`, all in one chain) in a new rack, as \
                 Group does: the rack takes the first device's place and holds one chain, named \
                 after the first device, that holds the devices in their order, each keeping \
                 its settings and running state; the rack is selected. The rack is the kind that \
                 can hold them: an audio effect rack for audio effects alone, a MIDI effect \
                 rack for MIDI effects alone, and an instrument rack for any other mix. A \
                 selection that grouping would put out of the chain's order is refused. With \
                 `chains` (several chains of one rack) they become the chains of a new rack of \
                 the same kind, inside a new chain in the first one's place. One undo step \
                 (Group).",
            )
            .permission(Permission::Edit)
            .contexts([DeviceView, RackChains])
            .binding("S-DEV-001")
            .args::<GroupArgs>()
            .result::<Grouped>(),
        CommandSpec::new(UNGROUP, "Ungroup")
            .description(
                "Takes the selected rack (or `device`) apart, as Ungroup does: the devices of \
                 its one chain take the rack's place in order and the first is selected; the \
                 rack, its chain's mixer and its Macro Controls go. A rack with no chains is \
                 just removed; a rack of several chains can't be taken apart. One undo step \
                 (Ungroup).",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .binding("S-DEV-002")
            .args::<DeviceArgs>()
            .result::<Ungrouped>(),
        CommandSpec::new(CHAIN_COPY, "Copy Chain")
            .description(
                "Copies a rack's chain, or the chains selected in the chain list if `chain` is \
                 left out, with their devices, names, colours, info texts and mixers, to the \
                 chain clipboard for `chain.paste`. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(RackChains)
            .binding("S-EDIT-002")
            .args::<ChainArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(CHAIN_CUT, "Cut Chain")
            .description(
                "Copies a rack's chain, or the selected chains if `chain` is left out, to the \
                 chain clipboard and deletes them: one undo step (Delete Chain). Because it \
                 removes the person's work an agent asks first.",
            )
            .permission(Permission::Destructive)
            .context(RackChains)
            .binding("S-EDIT-001")
            .args::<ChainArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(CHAIN_PASTE, "Paste Chain")
            .description(
                "Pastes the chain clipboard's chains into a rack at `index` (the end if left \
                 out), as new chains with new IDs and everything else copied: one undo step \
                 (Insert Chain). A chain must fit the rack's kind.",
            )
            .permission(Permission::Edit)
            .context(RackChains)
            .binding("S-EDIT-003")
            .args::<PasteChainArgs>()
            .result::<ChainAdded>(),
        CommandSpec::new(CHAIN_DUPLICATE, "Duplicate Chain")
            .description(
                "Puts a copy of a rack's chain, or of each selected chain if `chain` is left \
                 out, with new IDs, straight after it: one undo step (Insert Chain).",
            )
            .permission(Permission::Edit)
            .context(RackChains)
            .binding("S-EDIT-004")
            .args::<ChainArgs>()
            .result::<ChainAdded>(),
        CommandSpec::new(CHAIN_DELETE, "Delete Chain")
            .description(
                "Deletes the chains selected in the chain list, or `chains`, with their \
                 devices and automation: one undo step (Delete Chain). Undo restores them, but \
                 because it removes the person's work an agent asks first. `chain.remove` \
                 deletes one chain by its ID.",
            )
            .permission(Permission::Destructive)
            .context(RackChains)
            .binding("S-EDIT-005")
            .args::<DeleteChainsArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(CHAIN_SELECT, "Select Chain")
            .description(
                "Selects chains in a rack's chain list, as clicking a row does, and gives the \
                 chain list the keyboard focus: `replace` selects only `chain`, `toggle` adds \
                 or removes it (Cmd-click, Ctrl-click), `range` selects every chain from the \
                 selected one to it (Shift-click), and `previous` and `next` step the selection \
                 up and down the list (Up and Down). The rack's devices view shows the selected \
                 chain's devices, or says how many chains are selected. It changes only the \
                 view: it isn't an undo step and isn't saved with the set.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<SelectChainArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(CHAIN_SOLO, "Solo Chain")
            .description(
                "Solos the selected chains, or `chain`, as a chain's Solo button does: it is \
                 the rack's only soloed chain, and soloing it again unsolos it; with `add` it \
                 adds or removes the chain and leaves the others as they are (Cmd-click, \
                 Ctrl-click). Other racks and the tracks are not touched. One undo step. \
                 `chain.set_solo` sets one chain's solo by its ID.",
            )
            .permission(Permission::Edit)
            .context(RackChains)
            .binding("S-SES-023")
            .args::<SoloChainArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(CHAIN_HOT_SWAP, "Hot-Swap Chain")
            .description(
                "Arms a chain for hot-swap, as its Hot-Swap button does, or disarms it; \
                 arming it ends the hot-swap of a device, and the other way round. Only the \
                 armed state is built so far: it shows on the chain's row in \
                 `device_view.state`. It changes only the view.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<HotSwapChainArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(CHAIN_CREATE, "Create Chain")
            .description(
                "Makes a chain in a rack at `index` (the end if left out), as the chain \
                 list's drop area does, named after what it holds, and selects it: holding a \
                 new `device`, a device made from a `preset` file, or the existing device \
                 `move_device` moved out of where it is; with none of them it is empty. The \
                 chain must fit the rack's kind. In a drum rack a chain made without `index` \
                 receives the first pad from the view's first that holds no chain, and with \
                 `returns` it is a return chain, after the others. One undo step (Insert \
                 Chain).",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<CreateChainArgs>()
            .result::<ChainAdded>(),
        CommandSpec::new(CHAIN_DROP_ON_TRACK, "Drop Chain on Track")
            .description(
                "Moves a chain's devices, in order and keeping their settings and running \
                 state, into a track's own chain at `index`, and deletes the chain, as \
                 dropping a chain on a track's Device View outside any rack does: the \
                 chain's mixer and zones stay behind. The devices must fit the track and \
                 Live's order. One undo step (Move Chain).",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<DropChainOnTrackArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(RACK_SET_VIEW, "Show or Hide a Rack's View")
            .description(
                "Shows or hides one of the views of a rack, the selected device or `rack`: \
                 the Macro Controls, the chain list (View > Show Chains and Hide Chains) or \
                 the selected chain's devices. With every view hidden the rack folds into \
                 its view column. It changes only the view: it isn't an undo step, and the \
                 set keeps which views each rack shows.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .binding(
                Binding::new("S-DEV-012")
                    .arg("view", "chain_list")
                    .arg("shown", true),
            )
            .binding(
                Binding::new("S-DEV-013")
                    .arg("view", "chain_list")
                    .arg("shown", false),
            )
            .args::<SetRackViewArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(DISPLAY, "Device Display")
            .description(
                "Reports what a device's display shows now, read from the display data its \
                 node publishes after each block and from its parameters: curves and spectra \
                 (an EQ's spectrum, its bands' responses and their sum per curve; a \
                 compressor's transfer curve), the markers on them (an EQ's band dots, a \
                 compressor's threshold and level), and single readings (a compressor's gain \
                 reduction and levels), each with its unit and how it reads; the view the \
                 display shows, if it has views; and whether an engine is publishing (`live`). \
                 Empty for a device whose type draws no display. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<DeviceArgs>()
            .result::<DeviceDisplay>(),
        CommandSpec::new(SET_DISPLAY_VIEW, "Set Display View")
            .description(
                "Chooses the view a device's display shows, as its view buttons do: a \
                 compressor's Transfer Curve, Activity, or Collapsed. Only a device whose \
                 display has views takes one, and only one of them. It changes only the view: \
                 it isn't an undo step, and `device_view.state` and `device.display` report the \
                 view chosen.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<SetDisplayViewArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(ARM_MAP, "Map")
            .description(
                "Lights or puts out a modulator slot's Map button, as clicking it does. While \
                 one is lit, every parameter in the set that can be mapped is tinted, and \
                 clicking one maps it to the slot (`modulator.map`); Esc, or the button again \
                 (`armed`: false), puts it out and changes nothing. Lighting a button puts out \
                 the one that was lit. It changes only the view: it isn't an undo step, and \
                 `device_view.state` reports the lit button in `armed_map`. Only a modulator \
                 has slots.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<ArmMapArgs>()
            .result::<DeviceViewState>(),
        CommandSpec::new(WINDOWS, "Floating Device Windows")
            .description(
                "Reports every floating device's window (IMP-14), on any track: the device and \
                 its track, the window's title, where it sits and how big it is (`frame`, in \
                 points, `null` until the app has placed it), its smallest size (the device's \
                 docked size under its title bar), whether it can grow (only a device with a \
                 display that grows can), and the device as its window draws it. \
                 `device.set_floating` floats and docks devices. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<FloatingWindows>(),
        CommandSpec::new(SET_WINDOW, "Move Device Window")
            .description(
                "Moves or resizes a floating device's window, as dragging it does: `frame` is \
                 its outer top-left corner and the size of its contents, in points. A size is \
                 kept within the window's smallest (the device's docked size) and, for a device \
                 that doesn't grow, held at it. The frame is saved with the set but isn't an \
                 undo step, so undo never moves a window instead of undoing an edit.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<SetWindowArgs>()
            .result::<FloatingWindows>(),
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
        for id in crate::pads::IDS {
            let spec = registry.get(id).unwrap_or_else(|| panic!("{id}"));
            assert!(spec.permission.is_some(), "{id}");
            assert!(spec.args.is_some() && spec.result.is_some(), "{id}");
            assert!(!spec.description.is_empty(), "{id}");
        }
        assert_eq!(registry.iter().count(), IDS.len() + crate::pads::IDS.len());
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
    }
}
