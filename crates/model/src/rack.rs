//! Racks (WP-5.1; the racks spec): devices that hold chains.
//!
//! A rack is a [`Device`] of kind [`DeviceKind::Rack`]. Its chains run side
//! by side on the rack's input, each through its own devices and its own
//! mixer ([`ChainMixer`]), and the rack's output is their sum. A chain's
//! devices may be racks too, so racks nest to any depth.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use bl_core::id::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::device::{Device, DeviceKind, DeviceRole};
use crate::error::ModelError;
use crate::ids::{ChainId, ContentHash, DeviceId, ParamId};
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

/// What every kind of rack describes of its parameters (device framework
/// R3): the chain selector and Chain Selector Filters MIDI Ctrl.
#[must_use]
pub fn description() -> &'static DeviceDescription {
    static DESCRIPTION: LazyLock<DeviceDescription> = LazyLock::new(|| {
        DeviceDescription::new(vec![
            ParamDescriptor::integer(
                CHAIN_SELECTOR,
                "Chain Selector",
                (0, 127),
                0,
                ParamUnit::None,
            ),
            ParamDescriptor::toggle(FILTER_MIDI_CTRL, "Chain Selector Filters MIDI Ctrl", false),
        ])
    });
    &DESCRIPTION
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
}

/// Which of a rack's views are shown (racks spec, R19): all of them in a new
/// rack. It's view state kept beside the set's document ([`RackViewMap`]), so
/// changing it is never an undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "Which views of a rack are shown: `macros` (the Macro Controls), `chain_list` \
                   and `devices` (the selected chain's devices), all on in a new rack; \
                   `zone_editor` (`key`, `velocity` or `select`, none in a new rack) and \
                   `auto_select` (off in a new rack)."
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
    /// (racks spec, R37). Off in a new rack.
    #[serde(default, skip_serializing_if = "is_off")]
    pub auto_select: bool,
}

impl Default for RackViews {
    fn default() -> Self {
        Self {
            macros: true,
            chain_list: true,
            devices: true,
            zone_editor: None,
            auto_select: false,
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
        }
    }

    /// The same views with `view` shown or hidden.
    #[must_use]
    pub fn with(mut self, view: RackView, shown: bool) -> Self {
        match view {
            RackView::Macros => self.macros = shown,
            RackView::ChainList => self.chain_list = shown,
            RackView::Devices => self.devices = shown,
        }
        self
    }

    /// Whether every view is hidden, so the rack folds into its view column
    /// (racks spec, R19).
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
        self.id = ChainId(next());
        for device in &mut self.devices {
            device.reassign_ids(next);
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

    /// Gives this device, every device inside it and every chain a new ID
    /// from `next`, in signal order: the copy of a rack that is pasted or
    /// duplicated, which docks every device it holds and gives each the
    /// active state in both of its states (device compare R8).
    pub fn reassign_ids(&mut self, next: &mut impl FnMut() -> Id) {
        self.id = DeviceId(next());
        self.floating = false;
        self.set_both(self.params.clone());
        for chain in &mut self.chains {
            chain.reassign_ids(next);
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
