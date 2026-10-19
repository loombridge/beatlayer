//! Racks (WP-5.1; the racks spec): devices that hold chains.
//!
//! A rack is a [`Device`] of kind [`DeviceKind::Rack`]. Its chains run side
//! by side on the rack's input, each through its own devices and its own
//! mixer ([`ChainMixer`]), and the rack's output is their sum. A chain's
//! devices may be racks too, so racks nest to any depth.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use bl_core::id::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::automation::MixerParam;
use crate::device::{Device, DeviceKind, DeviceRole};
use crate::error::ModelError;
use crate::ids::{ChainId, ContentHash, DeviceId, ParamId, TrackId};
use crate::layers::{Zone, ZoneKind};
use crate::param::{DeviceDescription, ParamDescriptor, ParamUnit};

/// How many colours a chain can take: the palette's 5 rows of 14 swatches
/// (C-TRACK-015).
pub const CHAIN_COLOURS: u8 = 70;

/// The rack's chain selector (racks spec, R33): a whole number from 0 to 127.
pub const CHAIN_SELECTOR: ParamId = ParamId(0);

/// Whether the chain select zones filter messages other than notes too
/// (Chain Selector Filters MIDI Ctrl, racks spec, R31).
pub const FILTER_MIDI_CTRL: ParamId = ParamId(1);

/// How many macro controls every rack has (macros spec, R1).
pub const MACROS: u8 = 16;

/// How many macros a new rack shows (macros spec, R2).
pub const SHOWN_MACROS: u8 = 8;

/// Macro 1's parameter number. Macro N is `MACRO_BASE + N - 1`.
pub const MACRO_BASE: u32 = 100;

/// The parameter number of macro `number`, numbered from 1.
#[must_use]
pub fn macro_param(number: u8) -> ParamId {
    ParamId(MACRO_BASE + u32::from(number.saturating_sub(1)))
}

/// Which macro a parameter number is, from 1, or `None` for another
/// parameter.
#[must_use]
pub fn macro_number(param: ParamId) -> Option<u8> {
    let offset = param.0.checked_sub(MACRO_BASE)?;
    (offset < u32::from(MACROS)).then(|| u8::try_from(offset).expect("below 16") + 1)
}

/// The name a macro has before Rename and without one mapping: its number
/// (macros spec, R10).
#[must_use]
pub fn default_macro_name(number: u8) -> String {
    format!("Macro {number}")
}

/// What every kind of rack describes of its parameters (device framework
/// R3): the chain selector, Chain Selector Filters MIDI Ctrl, and the 16
/// macros (macros spec, R1).
#[must_use]
pub fn description() -> &'static DeviceDescription {
    static DESCRIPTION: LazyLock<DeviceDescription> = LazyLock::new(|| {
        let mut params = vec![
            ParamDescriptor::integer(
                CHAIN_SELECTOR,
                "Chain Selector",
                (0, 127),
                0,
                ParamUnit::None,
            ),
            ParamDescriptor::toggle(FILTER_MIDI_CTRL, "Chain Selector Filters MIDI Ctrl", false),
        ];
        for number in 1..=MACROS {
            params.push(ParamDescriptor::continuous(
                macro_param(number),
                &default_macro_name(number),
                (0.0, 1.0),
                0.0,
                ParamUnit::None,
            ));
        }
        DeviceDescription::new(params)
    });
    &DESCRIPTION
}

/// One macro's saved settings beside its value (macros spec, R10, R14 and
/// R16). A macro's value is a parameter of its rack, in `Device::params`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Macro {
    /// The name Rename gave it; empty for its default name (R10).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// A swatch of the palette of 5 rows of 14, from 0 to 69, or none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<u8>,
    /// The info text the user gave it (Edit Info Text).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub info: String,
    /// Whether Randomize leaves it alone (R16).
    #[serde(default, skip_serializing_if = "is_off")]
    pub randomize_excluded: bool,
}

/// A parameter a macro drives: a device's parameter inside the rack, or a
/// chain's mixer control (macros spec, R4).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "What a macro maps: `device` with the device and its `param`, or `chain` with \
                   a rack's chain and its mixer `param` (volume, pan, or mute, the Chain \
                   Activator)."
)]
pub enum MacroTarget {
    /// A parameter of a device inside the rack, at any depth.
    Device {
        /// The device.
        device: DeviceId,
        /// Its parameter.
        param: ParamId,
    },
    /// A control of a chain's mixer.
    Chain {
        /// The chain.
        chain: ChainId,
        /// The control.
        param: MixerParam,
    },
}

/// One mapping: a macro drives a parameter over a range (macros spec, R9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MacroMapping {
    /// The macro, numbered from 1.
    pub macro_number: u8,
    /// What it drives.
    pub target: MacroTarget,
    /// The bottom of the range, as a position on the target's travel from 0
    /// to 1 (R7).
    pub min: f64,
    /// The top of the range, as a position on the target's travel.
    pub max: f64,
}

impl MacroMapping {
    /// The position on its target's travel that macro position `value` gives
    /// (macros spec, R7): `min + value · (max − min)`.
    #[must_use]
    pub fn position(&self, value: f64) -> f64 {
        self.min + value * (self.max - self.min)
    }
}

/// A rack's macros' settings and mappings (macros spec, R17). The values
/// themselves are the rack's parameters.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RackMacros {
    /// Each macro's settings, in macro order; a macro left off keeps the
    /// defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<Macro>,
    /// The mappings, in the order they were made (R9).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mappings: Vec<MacroMapping>,
}

impl RackMacros {
    /// Whether the rack has no settings or mappings to save.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.settings.is_empty() && self.mappings.is_empty()
    }

    /// Macro `number`'s settings, as it starts if it has none.
    #[must_use]
    pub fn setting(&self, number: u8) -> &Macro {
        static DEFAULT: Macro = Macro {
            name: String::new(),
            colour: None,
            info: String::new(),
            randomize_excluded: false,
        };
        self.settings
            .get(usize::from(number.saturating_sub(1)))
            .unwrap_or(&DEFAULT)
    }

    /// Macro `number`'s settings, to change, growing the list to reach it.
    pub fn setting_mut(&mut self, number: u8) -> &mut Macro {
        let at = usize::from(number.saturating_sub(1));
        if self.settings.len() <= at {
            self.settings.resize(at + 1, Macro::default());
        }
        &mut self.settings[at]
    }

    /// The mapping of `target`, if the rack has one.
    #[must_use]
    pub fn mapping(&self, target: MacroTarget) -> Option<&MacroMapping> {
        self.mappings.iter().find(|m| m.target == target)
    }

    /// The mappings of macro `number`, in the order they were made.
    pub fn mappings_of(&self, number: u8) -> impl Iterator<Item = &MacroMapping> {
        self.mappings
            .iter()
            .filter(move |m| m.macro_number == number)
    }

    /// Whether macro `number` drives anything.
    #[must_use]
    pub fn has_mappings(&self, number: u8) -> bool {
        self.mappings.iter().any(|m| m.macro_number == number)
    }
}

/// The four kinds of rack. A rack has the role of what it holds
/// ([`RackKind::role`]).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RackKind {
    /// Holds MIDI effects, an instrument and audio effects in each chain.
    Instrument,
    /// Holds audio effects only.
    AudioEffect,
    /// Holds MIDI effects only.
    MidiEffect,
    /// An instrument rack whose chains are keyed to notes (WP-5.6).
    Drum,
}

impl RackKind {
    /// Where a rack of this kind goes in a chain: an audio effect rack is an
    /// audio effect, a MIDI effect rack a MIDI effect, and the other two are
    /// instruments (racks spec, R1).
    #[must_use]
    pub fn role(self) -> DeviceRole {
        match self {
            Self::Instrument | Self::Drum => DeviceRole::Instrument,
            Self::AudioEffect => DeviceRole::AudioEffect,
            Self::MidiEffect => DeviceRole::MidiEffect,
        }
    }

    /// The name a new rack of this kind has.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Instrument => "Instrument Rack",
            Self::AudioEffect => "Audio Effect Rack",
            Self::MidiEffect => "MIDI Effect Rack",
            Self::Drum => "Drum Rack",
        }
    }

    /// The zones a chain of this kind of rack has (racks spec, R30): all
    /// three, only chain select for an audio effect rack, none for a drum
    /// rack.
    #[must_use]
    pub fn zone_kinds(self) -> &'static [ZoneKind] {
        match self {
            Self::Instrument | Self::MidiEffect => {
                &[ZoneKind::Key, ZoneKind::Velocity, ZoneKind::Select]
            }
            Self::AudioEffect => &[ZoneKind::Select],
            Self::Drum => &[],
        }
    }

    /// Whether a chain of this kind of rack has Volume and Pan: all but a
    /// MIDI effect rack's (racks spec, R5).
    #[must_use]
    pub fn chains_have_volume_and_pan(self) -> bool {
        self != Self::MidiEffect
    }

    /// The rack kind that can hold a selection of devices with these roles
    /// (racks spec, R9): an audio effect rack for audio effects alone, a MIDI
    /// effect rack for MIDI effects alone, and an instrument rack for any
    /// other selection.
    #[must_use]
    pub fn holding(roles: impl IntoIterator<Item = DeviceRole>) -> Self {
        let roles: Vec<DeviceRole> = roles.into_iter().collect();
        let all = |role| !roles.is_empty() && roles.iter().all(|r| *r == role);
        if all(DeviceRole::AudioEffect) {
            Self::AudioEffect
        } else if all(DeviceRole::MidiEffect) {
            Self::MidiEffect
        } else {
            Self::Instrument
        }
    }
}

/// A chain's mixer: Volume, Pan, the Chain Activator and Solo (racks spec,
/// R5). A MIDI effect rack's chains keep Volume and Pan at their defaults.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A chain's mixer: `volume` (linear gain, 1 is 0 dB, 0 is -inf), `pan` (-1 left \
                   to 1 right), `enabled` (the Chain Activator) and `solo`. A MIDI effect rack's \
                   chains have no volume or pan."
)]
pub struct ChainMixer {
    /// Linear gain: 1 (0 dB) in a new chain.
    #[serde(default = "unity", skip_serializing_if = "is_unity")]
    pub volume: f64,
    /// From −1 (left) to 1 (right): centre in a new chain.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pan: f64,
    /// The Chain Activator: on in a new chain. A chain that's off adds
    /// nothing to its rack's output.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub enabled: bool,
    /// Solo: while any chain of a rack is soloed, its unsoloed chains add
    /// nothing.
    #[serde(default, skip_serializing_if = "is_off")]
    pub solo: bool,
}

impl Default for ChainMixer {
    fn default() -> Self {
        Self {
            volume: 1.0,
            pan: 0.0,
            enabled: true,
            solo: false,
        }
    }
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_unity(value: &f64) -> bool {
    *value == 1.0
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_on(value: &bool) -> bool {
    *value
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_off(value: &bool) -> bool {
    !*value
}

fn unity() -> f64 {
    1.0
}

fn on() -> bool {
    true
}

/// A chain's three zones (racks spec, R30). A new chain passes everything:
/// every note, every velocity, and the chain select value 0 alone, with no
/// fades.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    description = "A chain's zones, each `low`, `high` and the `fade_low` and `fade_high` values \
                   its fades span: `key` (0 to 127), `velocity` (1 to 127) and `select` (the \
                   chain select zone, 0 to 127). One left out passes everything: every note, \
                   every velocity, and chain select 0 alone."
)]
pub struct ChainZones {
    /// The key zone.
    #[serde(default = "whole_keys", skip_serializing_if = "is_whole_keys")]
    pub key: Zone,
    /// The velocity zone.
    #[serde(
        default = "whole_velocities",
        skip_serializing_if = "is_whole_velocities"
    )]
    pub velocity: Zone,
    /// The chain select zone.
    #[serde(default = "select_zero", skip_serializing_if = "is_select_zero")]
    pub select: Zone,
}

fn whole_keys() -> Zone {
    Zone::whole(ZoneKind::Key)
}

fn whole_velocities() -> Zone {
    Zone::whole(ZoneKind::Velocity)
}

fn select_zero() -> Zone {
    Zone::new(0, 0)
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_whole_keys(zone: &Zone) -> bool {
    *zone == whole_keys()
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_whole_velocities(zone: &Zone) -> bool {
    *zone == whole_velocities()
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_select_zero(zone: &Zone) -> bool {
    *zone == select_zero()
}

impl Default for ChainZones {
    fn default() -> Self {
        Self {
            key: whole_keys(),
            velocity: whole_velocities(),
            select: select_zero(),
        }
    }
}

impl ChainZones {
    /// The zone of this kind.
    #[must_use]
    pub fn get(&self, kind: ZoneKind) -> Zone {
        match kind {
            ZoneKind::Key => self.key,
            ZoneKind::Velocity => self.velocity,
            ZoneKind::Select => self.select,
        }
    }

    /// The same zones with this kind's replaced.
    #[must_use]
    pub fn with(mut self, kind: ZoneKind, zone: Zone) -> Self {
        match kind {
            ZoneKind::Key => self.key = zone,
            ZoneKind::Velocity => self.velocity = zone,
            ZoneKind::Select => self.select = zone,
        }
        self
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// One of the views a rack's frame can show (racks spec, R18).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RackView {
    /// The Macro Controls ([macros](../../docs/specs/macros.md), WP-5.4).
    Macros,
    /// The chain list.
    ChainList,
    /// The selected chain's devices.
    Devices,
    /// A drum rack's pad view (drum racks spec, R11).
    Pads,
    /// A drum rack's Input/Output switch (R21).
    InputOutput,
    /// A drum rack's Send switch.
    Send,
    /// A drum rack's Return switch.
    Returns,
    /// A drum rack's Auto Select switch.
    AutoSelect,
}

/// Which of a rack's views are shown (racks spec, R19): all of them in a new
/// rack. It's view state kept beside the set's document ([`RackViewMap`]), so
/// changing it is never an undo step. A drum rack has more (drum racks spec,
/// R11, R21): the pad view, which notes it shows, and the switches that
/// choose the chain list's columns and its return section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "Which views of a rack are shown: `macros` (the Macro Controls), `chain_list` \
                   and `devices` (the selected chain's devices). All on in a new rack. A drum \
                   rack also has `pads` (the pad view, on in a new rack) with `first_pad` (the \
                   lowest note it shows, a multiple of 4, 36 in a new rack), and the switches \
                   `input_output` (each chain's Preview, Receive, Play and Choke), `send` (its \
                   send sliders) and `returns` (the return chains' section), all off in a new \
                   rack. Every rack has `zone_editor` (`key`, `velocity` or `select`, none in a \
                   new rack; not a drum rack) and `auto_select` (the chain list follows the \
                   notes, off in a new rack)."
)]
pub struct RackViews {
    /// The Macro Controls.
    #[serde(default = "on")]
    pub macros: bool,
    /// The chain list.
    #[serde(default = "on")]
    pub chain_list: bool,
    /// The selected chain's devices.
    #[serde(default = "on")]
    pub devices: bool,
    /// The zone editor beside the chain list (racks spec, R35): the Key,
    /// Vel (`velocity`) or Chain (`select`) editor, or none while Hide is
    /// on, as in a new rack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_editor: Option<ZoneKind>,
    /// Auto Select: the chain list selects the chains that are processing
    /// (racks spec, R37; drum racks follow their notes). Off in a new rack.
    #[serde(default, skip_serializing_if = "is_off")]
    pub auto_select: bool,
    /// A drum rack's pad view.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub pads: bool,
    /// The lowest note the pad view shows: 36 (C1) in a new drum rack.
    #[serde(default = "first_pad", skip_serializing_if = "is_first_pad")]
    pub first_pad: u8,
    /// A drum rack's Input/Output switch.
    #[serde(default, skip_serializing_if = "is_off")]
    pub input_output: bool,
    /// A drum rack's Send switch.
    #[serde(default, skip_serializing_if = "is_off")]
    pub send: bool,
    /// A drum rack's Return switch.
    #[serde(default, skip_serializing_if = "is_off")]
    pub returns: bool,
    /// How many macros the Macro Controls show, from 1 to 16 (macros spec,
    /// R2): 8 in a new rack.
    #[serde(default = "shown_macros", skip_serializing_if = "is_shown_macros")]
    pub macro_count: u8,
}

/// The first note a new drum rack's pad view shows (drum racks spec, R11).
pub const FIRST_PAD: u8 = 36;

fn first_pad() -> u8 {
    FIRST_PAD
}

fn shown_macros() -> u8 {
    SHOWN_MACROS
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_shown_macros(value: &u8) -> bool {
    *value == SHOWN_MACROS
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_first_pad(value: &u8) -> bool {
    *value == FIRST_PAD
}

impl Default for RackViews {
    fn default() -> Self {
        Self {
            macros: true,
            chain_list: true,
            devices: true,
            zone_editor: None,
            pads: true,
            first_pad: FIRST_PAD,
            input_output: false,
            send: false,
            returns: false,
            auto_select: false,
            macro_count: SHOWN_MACROS,
        }
    }
}

impl RackViews {
    /// Whether `view` is shown.
    #[must_use]
    pub fn shown(self, view: RackView) -> bool {
        match view {
            RackView::Macros => self.macros,
            RackView::ChainList => self.chain_list,
            RackView::Devices => self.devices,
            RackView::Pads => self.pads,
            RackView::InputOutput => self.input_output,
            RackView::Send => self.send,
            RackView::Returns => self.returns,
            RackView::AutoSelect => self.auto_select,
        }
    }

    /// The same views with `view` shown or hidden.
    #[must_use]
    pub fn with(mut self, view: RackView, shown: bool) -> Self {
        match view {
            RackView::Macros => self.macros = shown,
            RackView::ChainList => self.chain_list = shown,
            RackView::Devices => self.devices = shown,
            RackView::Pads => self.pads = shown,
            RackView::InputOutput => self.input_output = shown,
            RackView::Send => self.send = shown,
            RackView::Returns => self.returns = shown,
            RackView::AutoSelect => self.auto_select = shown,
        }
        self
    }

    /// Whether every view every rack has is hidden, so the rack folds into
    /// its view column (racks spec, R19). A drum rack's pad view counts too,
    /// which only the rack's frame knows; the switches only decide what the
    /// chain list shows.
    #[must_use]
    pub fn all_hidden(self) -> bool {
        !(self.macros || self.chain_list || self.devices)
    }
}

/// Which views each rack shows, as a set saves them beside its document
/// (racks spec, R19). A rack with every view shown has no entry.
pub type RackViewMap = std::collections::BTreeMap<DeviceId, RackViews>;

/// Where a device sits: in its track's chain (`chain` is `None`) or in a
/// rack's chain, and at which position, counting from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevicePlace {
    /// The track whose chain holds the device, directly or in a rack.
    pub track: crate::ids::TrackId,
    /// The rack chain the device is in, if it is in one.
    pub chain: Option<ChainId>,
    /// Its position in that chain or the track's.
    pub index: usize,
}

/// One chain of a rack: its own name, colour, info text and mixer, and its
/// devices in signal order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Chain {
    /// The chain's ID.
    pub id: ChainId,
    /// The chain's name.
    pub name: String,
    /// The chain's colour: an index into the palette of 5 rows of 14
    /// swatches, from 0 to 69, or none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<u8>,
    /// The info text the user gave the chain (Edit Info Text).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub info: String,
    /// The chain's mixer.
    #[serde(default)]
    pub mixer: ChainMixer,
    /// The chain's zones. Left out of the saved set while they pass
    /// everything.
    #[serde(default, skip_serializing_if = "ChainZones::is_default")]
    pub zones: ChainZones,
    /// The chain's devices, in signal order.
    #[serde(default)]
    pub devices: Vec<Device>,
    /// The note a drum rack's chain answers (drum racks spec, R1). Only a
    /// drum rack's chains have one; any other chain keeps the default.
    #[serde(default, skip_serializing_if = "Receive::is_default")]
    pub receive: Receive,
    /// The note a drum rack's chain passes on to its devices, from 0 to 127:
    /// C3 (60) in a new chain.
    #[serde(default = "play", skip_serializing_if = "is_play")]
    pub play: u8,
    /// The choke group a drum rack's chain is in, from 1 to 16, or none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choke: Option<u8>,
    /// A drum rack's drum chain's sends to its return chains, post-fader;
    /// a return chain without an entry gets none (−∞ dB).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<ChainSend>,
    /// Whether this is one of a drum rack's return chains, which sit after
    /// its drum chains (drum racks spec, R7).
    #[serde(default, skip_serializing_if = "is_off")]
    pub is_return: bool,
    /// Where a return chain's output goes: one of the set's return tracks,
    /// or the rack's own output while there is none (drum racks spec, R8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_to: Option<TrackId>,
}

/// The note a drum rack's chain answers (drum racks spec, R1).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "What a drum chain answers: `all_notes`, or `{\"note\": n}` with a MIDI note \
                   from 0 to 127 (36 is C1, which a new chain on the first pad answers)."
)]
pub enum Receive {
    /// Every note, passed on as it came.
    AllNotes,
    /// One note.
    Note(u8),
}

impl Default for Receive {
    fn default() -> Self {
        Self::Note(FIRST_PAD)
    }
}

impl Receive {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether a note-on of `key` reaches a chain with this Receive.
    #[must_use]
    pub fn answers(self, key: u8) -> bool {
        match self {
            Self::AllNotes => true,
            Self::Note(note) => note == key,
        }
    }
}

/// A note's name in Live's octave numbering, where note 60 is C3 and note 36
/// is C1: `C-2` for 0, `F#3` for 66, `G8` for 127.
#[must_use]
pub fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = i16::from(note / 12) - 2;
    format!("{}{octave}", NAMES[usize::from(note % 12)])
}

/// The General MIDI drum sound on `note`, from 35 to 81, if it has one.
#[must_use]
pub fn general_midi_drum(note: u8) -> Option<&'static str> {
    const SOUNDS: [&str; 47] = [
        "Acoustic Bass Drum",
        "Bass Drum 1",
        "Side Stick",
        "Acoustic Snare",
        "Hand Clap",
        "Electric Snare",
        "Low Floor Tom",
        "Closed Hi-Hat",
        "High Floor Tom",
        "Pedal Hi-Hat",
        "Low Tom",
        "Open Hi-Hat",
        "Low-Mid Tom",
        "Hi-Mid Tom",
        "Crash Cymbal 1",
        "High Tom",
        "Ride Cymbal 1",
        "Chinese Cymbal",
        "Ride Bell",
        "Tambourine",
        "Splash Cymbal",
        "Cowbell",
        "Crash Cymbal 2",
        "Vibraslap",
        "Ride Cymbal 2",
        "Hi Bongo",
        "Low Bongo",
        "Mute Hi Conga",
        "Open Hi Conga",
        "Low Conga",
        "High Timbale",
        "Low Timbale",
        "High Agogo",
        "Low Agogo",
        "Cabasa",
        "Maracas",
        "Short Whistle",
        "Long Whistle",
        "Short Guiro",
        "Long Guiro",
        "Claves",
        "Hi Wood Block",
        "Low Wood Block",
        "Mute Cuica",
        "Open Cuica",
        "Mute Triangle",
        "Open Triangle",
    ];
    note.checked_sub(35)
        .and_then(|at| SOUNDS.get(usize::from(at)))
        .copied()
}

/// What a Receive chooser entry reads (drum racks spec, R3): All Notes, or a
/// note's name, its number and its General MIDI drum sound where it has one.
#[must_use]
pub fn receive_label(receive: Receive) -> String {
    match receive {
        Receive::AllNotes => "All Notes".to_owned(),
        Receive::Note(note) => match general_midi_drum(note) {
            Some(sound) => format!("{} ({note}) {sound}", note_name(note)),
            None => format!("{} ({note})", note_name(note)),
        },
    }
}

/// The entries of a Receive chooser, in order: All Notes first, then each
/// note from 0 to 127 with its label (drum racks spec, R3).
#[must_use]
pub fn receive_choices() -> Vec<(Receive, String)> {
    std::iter::once(Receive::AllNotes)
        .chain((0..128).map(Receive::Note))
        .map(|receive| (receive, receive_label(receive)))
        .collect()
}

/// A drum chain's send to a return chain (drum racks spec, R6).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A send to the return chain `to`, as linear gain (1 is 0 dB, 0 is -inf), \
                   taken after the chain's Volume, Pan and activator."
)]
pub struct ChainSend {
    /// The return chain.
    pub to: ChainId,
    /// The level as linear gain: 0 in a new send.
    pub level: f64,
}

/// How many return chains a drum rack holds (drum racks spec, R7).
pub const MAX_RETURN_CHAINS: usize = 6;

/// The note a new drum chain plays: C3, which plays a sampler's sample at its
/// own pitch (drum racks spec, R3).
pub const DEFAULT_PLAY: u8 = 60;

fn play() -> u8 {
    DEFAULT_PLAY
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_play(value: &u8) -> bool {
    *value == DEFAULT_PLAY
}

impl Chain {
    /// A new drum chain for the pad `note`: it receives `note` and plays C3
    /// (drum racks spec, R3).
    #[must_use]
    pub fn for_pad(id: ChainId, name: impl Into<String>, note: u8) -> Self {
        let mut chain = Self::new(id, name);
        chain.receive = Receive::Note(note);
        chain
    }

    /// A new return chain, sent to the rack's own output.
    #[must_use]
    pub fn new_return(id: ChainId, name: impl Into<String>) -> Self {
        let mut chain = Self::new(id, name);
        chain.is_return = true;
        chain
    }

    /// The level of the send to the return chain `to`, as linear gain.
    #[must_use]
    pub fn send(&self, to: ChainId) -> f64 {
        self.sends
            .iter()
            .find(|s| s.to == to)
            .map_or(0.0, |s| s.level)
    }

    /// Whether the chain holds only what any chain holds: no note, choke
    /// group or send of a drum rack's.
    fn has_no_drum_settings(&self) -> bool {
        self.receive.is_default()
            && self.play == DEFAULT_PLAY
            && self.choke.is_none()
            && self.sends.is_empty()
            && !self.is_return
            && self.audio_to.is_none()
    }

    /// Makes the chain fit `rack` after it moved there: a drum rack keeps
    /// the sends to its own return chains, any other rack's chain loses
    /// every setting only a drum rack's chains have.
    pub fn fit_to(&mut self, rack: &Device) {
        if rack.rack_kind() == Some(RackKind::Drum) {
            self.sends
                .retain(|s| rack.chains.iter().any(|c| c.is_return && c.id == s.to));
            if self.is_return {
                self.sends.clear();
            }
        } else {
            self.receive = Receive::default();
            self.play = DEFAULT_PLAY;
            self.choke = None;
            self.sends.clear();
            self.is_return = false;
        }
        if !self.is_return {
            self.audio_to = None;
        }
    }
}

impl Chain {
    /// An empty chain with a new chain's mixer.
    #[must_use]
    pub fn new(id: ChainId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            colour: None,
            info: String::new(),
            mixer: ChainMixer::default(),
            zones: ChainZones::default(),
            devices: Vec::new(),
            receive: Receive::default(),
            play: DEFAULT_PLAY,
            choke: None,
            sends: Vec::new(),
            is_return: false,
            audio_to: None,
        }
    }

    /// The same chain holding `devices`.
    #[must_use]
    pub fn with_devices(mut self, devices: Vec<Device>) -> Self {
        self.devices = devices;
        self
    }

    /// Every ID the chain and everything in it use: its own, and its
    /// devices' and their chains'.
    #[must_use]
    pub fn ids(&self) -> Vec<Id> {
        let mut ids = vec![self.id.0];
        ids.extend(self.devices.iter().flat_map(Device::ids));
        ids
    }

    /// Gives the chain and everything in it new IDs from `next`.
    pub fn reassign_ids(&mut self, next: &mut impl FnMut() -> Id) {
        self.reassign_ids_tracked(next, &mut BTreeMap::new(), &mut BTreeMap::new());
    }

    /// Gives the chain and everything in it new IDs, remembering each old
    /// ID's new one so a rack's macro mappings follow their targets
    /// (macros spec, R18).
    fn reassign_ids_tracked(
        &mut self,
        next: &mut impl FnMut() -> Id,
        devices: &mut BTreeMap<DeviceId, DeviceId>,
        chains: &mut BTreeMap<ChainId, ChainId>,
    ) {
        let old = self.id;
        self.id = ChainId(next());
        chains.insert(old, self.id);
        for device in &mut self.devices {
            device.reassign_ids_tracked(next, devices, chains);
        }
    }

    /// The samples the chain's devices play ([`Device::media_used`]).
    #[must_use]
    pub fn media_used(&self) -> BTreeSet<ContentHash> {
        self.devices.iter().flat_map(Device::media_used).collect()
    }

    /// Every device in the chain, those inside its racks included, in
    /// signal order.
    pub fn devices_deep(&self) -> impl Iterator<Item = &Device> {
        self.devices.iter().flat_map(Device::walk)
    }
}

impl Device {
    /// A switched-on rack of `kind` with no chains, named after its kind.
    #[must_use]
    pub fn rack(id: DeviceId, kind: RackKind) -> Self {
        let mut rack = Self::new(id, kind.name(), DeviceKind::Rack { kind });
        rack.role = kind.role();
        rack
    }

    /// The same rack with a chain added at the end.
    #[must_use]
    pub fn with_chain(mut self, chain: Chain) -> Self {
        self.chains.push(chain);
        self
    }

    /// The rack's kind, if the device is a rack.
    #[must_use]
    pub fn rack_kind(&self) -> Option<RackKind> {
        match self.kind {
            DeviceKind::Rack { kind } => Some(kind),
            _ => None,
        }
    }

    /// This device and every device inside it, at any depth, in signal
    /// order: a rack first, then its first chain's devices, and so on.
    #[must_use]
    pub fn walk(&self) -> Walk<'_> {
        Walk { stack: vec![self] }
    }

    /// The samples this device and everything in it play, by content hash:
    /// sample slots and multisampler layers.
    #[must_use]
    pub fn media_used(&self) -> BTreeSet<ContentHash> {
        self.walk()
            .flat_map(|d| {
                let layers = d.layers.list.iter().map(|layer| layer.media);
                d.media.iter().flatten().copied().chain(layers)
            })
            .collect()
    }

    /// The device with this ID, this one or one inside it.
    #[must_use]
    pub fn find(&self, id: DeviceId) -> Option<&Device> {
        self.walk().find(|d| d.id == id)
    }

    /// The device with this ID, to change in place.
    pub(crate) fn find_mut(&mut self, id: DeviceId) -> Option<&mut Device> {
        if self.id == id {
            return Some(self);
        }
        self.chains
            .iter_mut()
            .flat_map(|chain| chain.devices.iter_mut())
            .find_map(|device| device.find_mut(id))
    }

    /// Every chain of this device and of the racks inside it, at any depth.
    pub fn chains_deep(&self) -> impl Iterator<Item = &Chain> {
        self.walk().flat_map(|device| device.chains.iter())
    }

    /// The chain with this ID, in this rack or one inside it.
    #[must_use]
    pub fn find_chain(&self, id: ChainId) -> Option<&Chain> {
        self.chains_deep().find(|chain| chain.id == id)
    }

    /// The chain with this ID, to change in place.
    pub(crate) fn find_chain_mut(&mut self, id: ChainId) -> Option<&mut Chain> {
        // Searched before borrowed, so the borrow checker sees one path.
        let at = self.chains.iter().position(|chain| chain.id == id);
        if let Some(at) = at {
            return self.chains.get_mut(at);
        }
        self.chains
            .iter_mut()
            .flat_map(|chain| chain.devices.iter_mut())
            .find_map(|device| device.find_chain_mut(id))
    }

    /// The rack whose chain this is: this device or one inside it.
    #[must_use]
    pub fn chain_rack(&self, id: ChainId) -> Option<&Device> {
        self.walk()
            .find(|device| device.chains.iter().any(|chain| chain.id == id))
    }

    /// Whether the device and everything in it are laid out as `other`'s
    /// are: the same devices and chains in the same places. Settings may
    /// differ.
    #[must_use]
    pub fn same_shape(&self, other: &Device) -> bool {
        self.id == other.id
            && self.kind == other.kind
            && self.chains.len() == other.chains.len()
            && self.chains.iter().zip(&other.chains).all(|(a, b)| {
                a.id == b.id
                    && a.is_return == b.is_return
                    && a.audio_to == b.audio_to
                    && a.devices.len() == b.devices.len()
                    && a.devices
                        .iter()
                        .zip(&b.devices)
                        .all(|(a, b)| a.same_shape(b))
            })
    }

    /// Calls `f` on this device and every device inside it.
    pub(crate) fn walk_mut(&mut self, f: &mut impl FnMut(&mut Device)) {
        f(self);
        for device in self.chains.iter_mut().flat_map(|c| c.devices.iter_mut()) {
            device.walk_mut(f);
        }
    }

    /// Takes the device with this ID out of the chain it sits in, in this
    /// rack or one inside it.
    pub(crate) fn remove_nested(&mut self, id: DeviceId) -> Option<Device> {
        for chain in &mut self.chains {
            if let Some(at) = chain.devices.iter().position(|d| d.id == id) {
                return Some(chain.devices.remove(at));
            }
            if let Some(device) = chain.devices.iter_mut().find_map(|d| d.remove_nested(id)) {
                return Some(device);
            }
        }
        None
    }

    /// Takes the chain with this ID out of the rack it belongs to, in this
    /// rack or one inside it.
    pub(crate) fn remove_chain(&mut self, id: ChainId) -> Option<Chain> {
        if let Some(at) = self.chains.iter().position(|c| c.id == id) {
            return Some(self.chains.remove(at));
        }
        self.chains
            .iter_mut()
            .flat_map(|c| c.devices.iter_mut())
            .find_map(|d| d.remove_chain(id))
    }

    /// Drops the sends of every drum rack in this device to return chains
    /// that are no longer in it (drum racks spec, R6).
    pub(crate) fn prune_sends(&mut self) {
        self.walk_mut(&mut |device| {
            if device.rack_kind() != Some(RackKind::Drum) {
                return;
            }
            let returns: Vec<ChainId> = device
                .chains
                .iter()
                .filter(|c| c.is_return)
                .map(|c| c.id)
                .collect();
            for chain in &mut device.chains {
                chain.sends.retain(|s| returns.contains(&s.to));
            }
        });
    }

    /// Gives this device, every device inside it and every chain a new ID
    /// from `next`, in signal order: the copy of a rack that is pasted or
    /// duplicated, which docks every device it holds and gives each the
    /// active state in both of its states (device compare R8).
    pub fn reassign_ids(&mut self, next: &mut impl FnMut() -> Id) {
        self.reassign_ids_tracked(next, &mut BTreeMap::new(), &mut BTreeMap::new());
    }

    /// [`reassign_ids`](Self::reassign_ids), remembering each old ID's new
    /// one so this rack's macro mappings follow the devices and chains they
    /// map, as a copied rack's must (macros spec, R18).
    fn reassign_ids_tracked(
        &mut self,
        next: &mut impl FnMut() -> Id,
        devices: &mut BTreeMap<DeviceId, DeviceId>,
        chains: &mut BTreeMap<ChainId, ChainId>,
    ) {
        let old_id = self.id;
        self.id = DeviceId(next());
        devices.insert(old_id, self.id);
        self.floating = false;
        self.set_both(self.params.clone());
        let old: Vec<ChainId> = self.chains.iter().map(|c| c.id).collect();
        for chain in &mut self.chains {
            chain.reassign_ids_tracked(next, devices, chains);
        }
        // A drum chain's sends follow its rack's return chains to their new
        // IDs.
        let new: Vec<ChainId> = self.chains.iter().map(|c| c.id).collect();
        for chain in &mut self.chains {
            for send in &mut chain.sends {
                if let Some(i) = old.iter().position(|&id| id == send.to) {
                    send.to = new[i];
                }
            }
        }
        for mapping in &mut self.macros.mappings {
            match &mut mapping.target {
                MacroTarget::Device { device, .. } => {
                    if let Some(new) = devices.get(device) {
                        *device = *new;
                    }
                }
                MacroTarget::Chain { chain, .. } => {
                    if let Some(new) = chains.get(chain) {
                        *chain = *new;
                    }
                }
            }
        }
    }

    /// Every ID this device and everything in it use: devices and chains.
    pub fn ids(&self) -> Vec<Id> {
        let mut ids = Vec::new();
        for device in self.walk() {
            ids.push(device.id.0);
            ids.extend(device.chains.iter().map(|chain| chain.id.0));
        }
        ids
    }
}

/// The devices of a device and everything in its chains, depth first in
/// signal order ([`Device::walk`]).
pub struct Walk<'a> {
    stack: Vec<&'a Device>,
}

impl<'a> Iterator for Walk<'a> {
    type Item = &'a Device;

    fn next(&mut self) -> Option<&'a Device> {
        let device = self.stack.pop()?;
        for chain in device.chains.iter().rev() {
            self.stack.extend(chain.devices.iter().rev());
        }
        Some(device)
    }
}

/// Checks a device's racks: roles that match their kinds, chains that are
/// in order, mixers in range, and IDs that are used once.
pub(crate) fn check_racks(device: &Device) -> Result<(), ModelError> {
    let mut seen = BTreeSet::new();
    for id in device.ids() {
        if !seen.insert(id) {
            return Err(ModelError::DuplicateId(id));
        }
    }
    for device in device.walk() {
        check_rack(device)?;
    }
    Ok(())
}

/// Checks one rack, not the racks inside it.
fn check_rack(device: &Device) -> Result<(), ModelError> {
    match device.rack_kind() {
        None if device.chains.is_empty() => return Ok(()),
        None => return Err(ModelError::RackRole(device.id)),
        Some(kind) if device.role != kind.role() => {
            return Err(ModelError::RackRole(device.id));
        }
        Some(_) => {}
    }
    check_order(device)?;
    check_drum_chains(device)?;
    check_macros(device)?;
    let kind = device.rack_kind().expect("checked above to be a rack");
    for chain in &device.chains {
        let mixer = &chain.mixer;
        crate::command::check_value(
            "chain volume",
            mixer.volume,
            mixer.volume.is_finite() && mixer.volume >= 0.0,
        )?;
        crate::command::check_value("chain pan", mixer.pan, (-1.0..=1.0).contains(&mixer.pan))?;
        if let Some(colour) = chain.colour {
            crate::command::check_value("chain colour", f64::from(colour), colour < CHAIN_COLOURS)?;
        }
        check_zones(kind, chain)?;
    }
    if device
        .rack_kind()
        .is_some_and(|k| !k.chains_have_volume_and_pan())
        && let Some(chain) = device
            .chains
            .iter()
            .find(|c| c.mixer.volume != 1.0 || c.mixer.pan != 0.0)
    {
        return Err(ModelError::ChainHasNoMixer(chain.id));
    }
    Ok(())
}

/// Checks what only a drum rack's chains have (drum racks spec, R1, R6, R7):
/// no other rack's chain has a note, a choke group, a send or a place to
/// send its output; a drum rack's return chains come last, are at most six,
/// and only hold audio effects; sends name the rack's return chains.
fn check_drum_chains(rack: &Device) -> Result<(), ModelError> {
    if rack.rack_kind() != Some(RackKind::Drum) {
        return match rack.chains.iter().find(|c| !c.has_no_drum_settings()) {
            Some(chain) => Err(ModelError::NotADrumChain(chain.id)),
            None => Ok(()),
        };
    }
    let first_return = rack
        .chains
        .iter()
        .position(|c| c.is_return)
        .unwrap_or(rack.chains.len());
    if rack.chains[first_return..].iter().any(|c| !c.is_return) {
        return Err(ModelError::ReturnChainOrder(rack.id));
    }
    if rack.chains.len() - first_return > MAX_RETURN_CHAINS {
        return Err(ModelError::TooManyReturnChains(rack.id));
    }
    for chain in &rack.chains {
        if let Receive::Note(note) = chain.receive {
            crate::command::check_value("receive note", f64::from(note), note < 128)?;
        }
        crate::command::check_value("play note", f64::from(chain.play), chain.play < 128)?;
        if let Some(group) = chain.choke {
            crate::command::check_value(
                "choke group",
                f64::from(group),
                (1..=16).contains(&group),
            )?;
        }
        for send in &chain.sends {
            crate::command::check_value(
                "send level",
                send.level,
                send.level.is_finite() && send.level >= 0.0,
            )?;
            if !rack.chains[first_return..].iter().any(|c| c.id == send.to) {
                return Err(ModelError::NoSuchChain(send.to));
            }
        }
        let distinct = chain
            .sends
            .iter()
            .enumerate()
            .all(|(i, s)| chain.sends[..i].iter().all(|o| o.to != s.to));
        if !distinct {
            return Err(ModelError::NotADrumChain(chain.id));
        }
        if chain.is_return
            && (!chain.sends.is_empty()
                || chain.receive != Receive::default()
                || chain.play != DEFAULT_PLAY
                || chain.choke.is_some())
        {
            return Err(ModelError::NotADrumChain(chain.id));
        }
        if !chain.is_return && chain.audio_to.is_some() {
            return Err(ModelError::NotADrumChain(chain.id));
        }
    }
    Ok(())
}

/// Checks that each chain of a rack holds only devices that fit its kind and
/// holds them in order (racks spec, R1): an audio effect rack's chain only
/// audio effects, a MIDI effect rack's only MIDI effects, and the others'
/// MIDI effects, then at most one instrument, then audio effects.
pub(crate) fn check_order(rack: &Device) -> Result<(), ModelError> {
    let Some(kind) = rack.rack_kind() else {
        return Ok(());
    };
    for chain in &rack.chains {
        let mut last = None;
        for device in &chain.devices {
            let fits = match kind {
                RackKind::AudioEffect => device.role == DeviceRole::AudioEffect,
                RackKind::Drum if chain.is_return => device.role == DeviceRole::AudioEffect,
                RackKind::MidiEffect => device.role == DeviceRole::MidiEffect,
                RackKind::Instrument | RackKind::Drum => true,
            };
            if !fits {
                return Err(ModelError::ChainDoesNotFit {
                    rack: rack.id,
                    role: device.role,
                });
            }
            let repeated =
                device.role == DeviceRole::Instrument && last == Some(DeviceRole::Instrument);
            if last.is_some_and(|last| device.role < last) || repeated {
                return Err(ModelError::ChainOrder { rack: rack.id });
            }
            last = Some(device.role);
        }
    }
    Ok(())
}

/// Checks a rack's macros (macros spec, R17): each macro number is 1 to 16,
/// each range is a position from 0 to 1, each colour is a swatch, each target
/// is inside the rack, and no target is mapped twice.
fn check_macros(rack: &Device) -> Result<(), ModelError> {
    let macros = &rack.macros;
    if macros.settings.len() > usize::from(MACROS) {
        return Err(ModelError::InvalidInput("a rack has 16 macros"));
    }
    for macro_settings in &macros.settings {
        if let Some(colour) = macro_settings.colour {
            crate::command::check_value("macro colour", f64::from(colour), colour < CHAIN_COLOURS)?;
        }
    }
    for (i, mapping) in macros.mappings.iter().enumerate() {
        crate::command::check_value(
            "macro number",
            f64::from(mapping.macro_number),
            (1..=MACROS).contains(&mapping.macro_number),
        )?;
        crate::command::check_value(
            "mapping min",
            mapping.min,
            mapping.min.is_finite() && (0.0..=1.0).contains(&mapping.min),
        )?;
        crate::command::check_value(
            "mapping max",
            mapping.max,
            mapping.max.is_finite() && (0.0..=1.0).contains(&mapping.max),
        )?;
        match mapping.target {
            MacroTarget::Device { device, param } => {
                if device == rack.id && macro_number(param).is_some() {
                    return Err(ModelError::InvalidInput(
                        "a macro can't map its own rack's macros",
                    ));
                }
                if rack.find(device).is_none() {
                    return Err(ModelError::InvalidInput(
                        "a macro can only map a device inside its rack",
                    ));
                }
            }
            MacroTarget::Chain { chain, .. } => {
                if rack.find_chain(chain).is_none() {
                    return Err(ModelError::InvalidInput(
                        "a macro can only map a chain inside its rack",
                    ));
                }
            }
        }
        if macros.mappings[..i]
            .iter()
            .any(|other| other.target == mapping.target)
        {
            return Err(ModelError::InvalidInput(
                "a parameter is mapped to one macro of its rack",
            ));
        }
    }
    Ok(())
}

/// Checks that each of a chain's zones is valid and that the chain has only
/// the zones its rack's kind gives it (racks spec, R30): the others stay as
/// a new chain has them.
fn check_zones(kind: RackKind, chain: &Chain) -> Result<(), ModelError> {
    for zone_kind in [ZoneKind::Key, ZoneKind::Velocity, ZoneKind::Select] {
        let zone = chain.zones.get(zone_kind);
        if !zone.is_valid(zone_kind) {
            return Err(ModelError::InvalidInput(
                "a chain's zone must hold a value of its scale, with its fades inside it",
            ));
        }
        if !kind.zone_kinds().contains(&zone_kind) && zone != ChainZones::default().get(zone_kind) {
            return Err(ModelError::ChainHasNoZone {
                chain: chain.id,
                kind: zone_kind,
            });
        }
    }
    Ok(())
}
