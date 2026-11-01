//! The instruments' modulation matrix as data (spec: instrument modulation
//! R1, R2, R8 to R11): what a device type declares (its sources,
//! destinations and route slots), the routes a device holds in its free
//! slots, and how the two are read back and resolved.
//!
//! Nothing here runs on the audio thread: the evaluator is the device
//! crate's, and the engine reads the free routes as [`FreeRoutes`], a fixed
//! size, so a change reaches the playing device without a new program.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ParamId;
use crate::device::Device;

/// The most route slots a device type declares.
pub const MAX_SLOTS: usize = 64;

/// The values a source takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Range {
    /// 0 to 1.
    Unipolar,
    /// −1 to 1.
    Bipolar,
}

/// Whether a source or destination has a value for each voice or one for
/// the whole instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// One for each voice.
    Voice,
    /// One for every voice.
    Global,
}

/// How often a destination's routes are worked out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rate {
    /// Every frame.
    Audio,
    /// Every frame with Hi-Quality on, else every 32 frames.
    Control,
}

/// How a destination combines its routes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Contributions add and move the value on its travel.
    Additive,
    /// Factors multiply the value.
    Multiplicative,
}

/// A source a device declares (R1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceDef {
    /// Its name.
    pub name: &'static str,
    /// The values it takes.
    pub range: Range,
    /// Per voice or global.
    pub scope: Scope,
}

/// A destination a device declares (R1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DestDef {
    /// Its name: its parameter's, or the movement's where it has none.
    pub name: &'static str,
    /// The parameter it moves; none for a movement no parameter has
    /// (Drift's Pitch Mod).
    pub param: Option<ParamId>,
    /// Per voice or global.
    pub scope: Scope,
    /// How often its routes are worked out.
    pub rate: Rate,
    /// How it combines its routes.
    pub kind: Kind,
}

/// Where a slot's source or destination comes from (R2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The type's: this source or destination, by its place in the list.
    Fixed(u8),
    /// A choice parameter's: 0 is Off, and n is the nth of the list.
    Chosen(ParamId),
    /// Set by the route commands.
    Free,
}

/// One route slot of a device type (R2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotDef {
    /// Where its source comes from.
    pub source: Pick,
    /// Where its destination comes from.
    pub dest: Pick,
    /// Its amount, a parameter from −100 % to 100 %.
    pub amount: ParamId,
}

/// What a slot is, as the readback says (R11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotKind {
    /// The type sets its destination.
    Fixed,
    /// The device's choosers set its destination.
    Chosen,
    /// The route commands set it.
    Free,
}

impl SlotDef {
    /// What the slot is: free when the commands set it, chosen when a
    /// chooser sets its destination, else fixed (a fixed slot's source may
    /// still be a chooser, as Drift's dedicated slots').
    #[must_use]
    pub fn kind(&self) -> SlotKind {
        match (self.source, self.dest) {
            (Pick::Free, Pick::Free) => SlotKind::Free,
            (_, Pick::Chosen(_)) => SlotKind::Chosen,
            _ => SlotKind::Fixed,
        }
    }
}

/// A device type's matrix: its sources, destinations and slots, and the
/// source and destination pairs it doesn't allow together (R1, R2, R8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatrixLayout {
    /// The sources, in the order a chooser lists them after Off.
    pub sources: Vec<SourceDef>,
    /// The destinations, in the order a chooser lists them after Off.
    pub dests: Vec<DestDef>,
    /// The slots, at most [`MAX_SLOTS`]; a slot's number is its place.
    pub slots: Vec<SlotDef>,
    /// Pairs, as places in `sources` and `dests`, that a free slot refuses.
    pub forbidden: Vec<(u8, u8)>,
}

/// A route a device holds in a free slot: saved with the set and with the
/// device's presets (R10). Its amount is the slot's parameter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModRoute {
    /// The slot's number.
    pub slot: u8,
    /// The source's name.
    pub source: String,
    /// The destination's name.
    pub destination: String,
}

/// Why a route command is refused (R8, R9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatrixError {
    /// The device has no modulation matrix.
    NoMatrix,
    /// There's no such slot.
    NoSlot(usize),
    /// The slot is fixed or chosen, not free.
    NotFree(usize),
    /// There's no source of that name.
    UnknownSource(String),
    /// There's no destination of that name.
    UnknownDestination(String),
    /// The device doesn't allow that source and destination together.
    Forbidden(String, String),
    /// Every free slot holds a route.
    Full,
    /// The slot holds no route.
    Empty(usize),
    /// No free slot routes that source to that destination.
    NoRoute(String, String),
    /// Neither a slot nor a source and destination were named.
    Unnamed,
}

impl std::fmt::Display for MatrixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMatrix => write!(f, "the device has no modulation matrix"),
            Self::NoSlot(n) => write!(f, "the device has no slot {n}"),
            Self::NotFree(n) => write!(
                f,
                "slot {n} is fixed or chosen, and only free slots take routes"
            ),
            Self::UnknownSource(name) => write!(f, "the device has no modulation source {name:?}"),
            Self::UnknownDestination(name) => {
                write!(f, "the device has no modulation destination {name:?}")
            }
            Self::Forbidden(source, dest) => {
                write!(f, "the device doesn't route {source:?} to {dest:?}")
            }
            Self::Full => write!(f, "every free slot of the device holds a route"),
            Self::Empty(n) => write!(f, "slot {n} holds no route"),
            Self::NoRoute(source, dest) => {
                write!(f, "no slot routes {source:?} to {dest:?}")
            }
            Self::Unnamed => write!(f, "name a slot, or a source and a destination"),
        }
    }
}

impl std::error::Error for MatrixError {}

/// The free routes as the engine reads them: for each slot, the source and
/// destination as places in the layout's lists, or [`FreeRoutes::NONE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FreeRoutes(pub [[u8; 2]; MAX_SLOTS]);

impl FreeRoutes {
    /// A slot with no route.
    pub const NONE: [u8; 2] = [u8::MAX; 2];
}

impl Default for FreeRoutes {
    fn default() -> Self {
        Self([Self::NONE; MAX_SLOTS])
    }
}

/// One slot as the `device` result's `matrix` reads it (R11).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SlotRow {
    /// The slot's number.
    pub slot: usize,
    /// Fixed, chosen or free.
    pub kind: SlotKind,
    /// Its source's name, or none while it's Off or empty.
    pub source: Option<&'static str>,
    /// Its destination's name, or none while it's Off or empty.
    pub destination: Option<&'static str>,
    /// Its amount's parameter.
    pub amount_param: ParamId,
    /// Its amount, in per cent.
    pub amount: f64,
}

/// A source as the `matrix` reads it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SourceRow {
    /// Its name.
    pub name: &'static str,
    /// Its range.
    pub range: Range,
    /// Per voice or global.
    pub scope: Scope,
}

/// A destination as the `matrix` reads it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DestRow {
    /// Its parameter, if it has one.
    pub param: Option<ParamId>,
    /// Its name.
    pub name: &'static str,
    /// Its rate.
    pub rate: Rate,
    /// Additive or multiplicative.
    pub kind: Kind,
}

/// What the `device` result's `matrix` holds (R11).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MatrixState {
    /// The device's sources.
    pub sources: Vec<SourceRow>,
    /// Its destinations.
    pub destinations: Vec<DestRow>,
    /// Every slot.
    pub slots: Vec<SlotRow>,
}

/// A choice parameter's value in `device`, or Off while it holds none.
fn chosen(device: &Device, param: ParamId) -> usize {
    device
        .params
        .get(&param)
        .map_or(0, |v| v.round().max(0.0) as usize)
}

impl MatrixLayout {
    /// The place of the source named `name`, ignoring case.
    #[must_use]
    pub fn source_named(&self, name: &str) -> Option<usize> {
        self.sources
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// The place of the destination named `name`, ignoring case.
    #[must_use]
    pub fn dest_named(&self, name: &str) -> Option<usize> {
        self.dests
            .iter()
            .position(|d| d.name.eq_ignore_ascii_case(name))
    }

    /// The route `device` holds in free slot `slot`, as places in the lists.
    fn held(&self, device: &Device, slot: usize) -> Option<(usize, usize)> {
        let route = device.routes.iter().find(|r| usize::from(r.slot) == slot)?;
        Some((
            self.source_named(&route.source)?,
            self.dest_named(&route.destination)?,
        ))
    }

    /// The slot a `set_route` writes (R8): `slot` where given, else the free
    /// slot that already routes `source` to `destination`, else the first
    /// free slot holding no route. Also the names as the layout spells
    /// them.
    ///
    /// # Errors
    ///
    /// The slot, source or destination doesn't exist, the slot isn't free,
    /// the pair is one the device refuses, or no free slot is empty.
    pub fn plan_set(
        &self,
        device: &Device,
        slot: Option<usize>,
        source: &str,
        destination: &str,
    ) -> Result<ModRoute, MatrixError> {
        let s = self
            .source_named(source)
            .ok_or_else(|| MatrixError::UnknownSource(source.to_owned()))?;
        let d = self
            .dest_named(destination)
            .ok_or_else(|| MatrixError::UnknownDestination(destination.to_owned()))?;
        if self.forbidden.contains(&(s as u8, d as u8)) {
            return Err(MatrixError::Forbidden(
                source.to_owned(),
                destination.to_owned(),
            ));
        }
        let free = |n: usize| {
            self.slots
                .get(n)
                .is_some_and(|def| def.kind() == SlotKind::Free)
        };
        let slot = match slot {
            Some(n) if n >= self.slots.len() => return Err(MatrixError::NoSlot(n)),
            Some(n) if !free(n) => return Err(MatrixError::NotFree(n)),
            Some(n) => n,
            None => (0..self.slots.len())
                .filter(|&n| free(n))
                .find(|&n| self.held(device, n) == Some((s, d)))
                .or_else(|| {
                    (0..self.slots.len())
                        .filter(|&n| free(n))
                        .find(|&n| self.held(device, n).is_none())
                })
                .ok_or(MatrixError::Full)?,
        };
        Ok(ModRoute {
            slot: slot as u8,
            source: self.sources[s].name.to_owned(),
            destination: self.dests[d].name.to_owned(),
        })
    }

    /// The slot a `clear_route` empties (R9): `slot` where given, else the
    /// free slot routing `source` to `destination`.
    ///
    /// # Errors
    ///
    /// The slot doesn't exist or isn't free, a name is unknown, or the
    /// slot holds no route.
    pub fn plan_clear(
        &self,
        device: &Device,
        slot: Option<usize>,
        source: Option<&str>,
        destination: Option<&str>,
    ) -> Result<u8, MatrixError> {
        let free = |n: usize| {
            self.slots
                .get(n)
                .is_some_and(|def| def.kind() == SlotKind::Free)
        };
        let n = match (slot, source, destination) {
            (Some(n), ..) if n >= self.slots.len() => return Err(MatrixError::NoSlot(n)),
            (Some(n), ..) if !free(n) => return Err(MatrixError::NotFree(n)),
            (Some(n), ..) => n,
            (None, Some(source), Some(destination)) => {
                let s = self
                    .source_named(source)
                    .ok_or_else(|| MatrixError::UnknownSource(source.to_owned()))?;
                let d = self
                    .dest_named(destination)
                    .ok_or_else(|| MatrixError::UnknownDestination(destination.to_owned()))?;
                (0..self.slots.len())
                    .filter(|&n| free(n))
                    .find(|&n| self.held(device, n) == Some((s, d)))
                    .ok_or_else(|| {
                        MatrixError::NoRoute(source.to_owned(), destination.to_owned())
                    })?
            }
            _ => return Err(MatrixError::Unnamed),
        };
        if self.held(device, n).is_none() {
            return Err(MatrixError::Empty(n));
        }
        Ok(n as u8)
    }

    /// The free routes `device` holds, as places; a route naming something
    /// the layout lacks (a set from another version) holds nothing.
    #[must_use]
    pub fn free_routes(&self, device: &Device) -> FreeRoutes {
        let mut out = FreeRoutes::default();
        for route in &device.routes {
            let slot = usize::from(route.slot);
            if slot < self.slots.len()
                && self.slots[slot].kind() == SlotKind::Free
                && let Some((s, d)) = self.held(device, slot)
            {
                out.0[slot] = [s as u8, d as u8];
            }
        }
        out
    }

    /// Every slot of the matrix as `device` has it (R11).
    #[must_use]
    pub fn state(&self, device: &Device) -> MatrixState {
        let pick =
            |pick: Pick, free: Option<usize>, names: &dyn Fn(usize) -> Option<&'static str>| {
                match pick {
                    Pick::Fixed(n) => names(usize::from(n)),
                    Pick::Chosen(param) => chosen(device, param).checked_sub(1).and_then(names),
                    Pick::Free => free.and_then(names),
                }
            };
        let slots = self
            .slots
            .iter()
            .enumerate()
            .map(|(slot, def)| {
                let route = (def.kind() == SlotKind::Free)
                    .then(|| self.held(device, slot))
                    .flatten();
                SlotRow {
                    slot,
                    kind: def.kind(),
                    source: pick(def.source, route.map(|r| r.0), &|n| {
                        self.sources.get(n).map(|s| s.name)
                    }),
                    destination: pick(def.dest, route.map(|r| r.1), &|n| {
                        self.dests.get(n).map(|d| d.name)
                    }),
                    amount_param: def.amount,
                    amount: device.params.get(&def.amount).copied().unwrap_or(0.0),
                }
            })
            .collect();
        MatrixState {
            sources: self
                .sources
                .iter()
                .map(|s| SourceRow {
                    name: s.name,
                    range: s.range,
                    scope: s.scope,
                })
                .collect(),
            destinations: self
                .dests
                .iter()
                .map(|d| DestRow {
                    param: d.param,
                    name: d.name,
                    rate: d.rate,
                    kind: d.kind,
                })
                .collect(),
            slots,
        }
    }
}

/// The longest name a route holds.
const MAX_NAME: usize = 64;

/// Whether `routes` are sound: each slot once, below [`MAX_SLOTS`], with
/// names that can be a source's and a destination's.
///
/// # Errors
///
/// A route breaks one of those.
pub fn check_routes(routes: &[ModRoute]) -> Result<(), crate::error::ModelError> {
    use crate::error::ModelError::InvalidInput;
    let mut seen = [false; MAX_SLOTS];
    for route in routes {
        let slot = usize::from(route.slot);
        if slot >= MAX_SLOTS || std::mem::replace(&mut seen[slot], true) {
            return Err(InvalidInput(
                "a route takes a slot below 64, and each slot holds one",
            ));
        }
        if [&route.source, &route.destination]
            .iter()
            .any(|name| name.is_empty() || name.len() > MAX_NAME)
        {
            return Err(InvalidInput("a route's source and destination are names"));
        }
    }
    Ok(())
}

impl crate::Document {
    /// Puts a route in slot `slot` of the instrument `device`'s modulation
    /// matrix (the instrument modulation spec, R8).
    pub(crate) fn set_mod_route(
        &mut self,
        device: crate::ids::DeviceId,
        slot: u8,
        source: &str,
        destination: &str,
    ) -> Result<(), crate::error::ModelError> {
        let mut routes = self.device_ref(device)?.routes.clone();
        routes.retain(|r| r.slot != slot);
        routes.push(ModRoute {
            slot,
            source: source.to_owned(),
            destination: destination.to_owned(),
        });
        routes.sort_by_key(|r| r.slot);
        check_routes(&routes)?;
        if routes != self.device_ref(device)?.routes {
            self.device_mut(device)?.routes = routes;
        }
        Ok(())
    }

    /// Empties slot `slot` of the instrument `device`'s modulation matrix
    /// (the instrument modulation spec, R9).
    pub(crate) fn clear_mod_route(
        &mut self,
        device: crate::ids::DeviceId,
        slot: u8,
    ) -> Result<(), crate::error::ModelError> {
        if self
            .device_ref(device)?
            .routes
            .iter()
            .any(|r| r.slot == slot)
        {
            self.device_mut(device)?.routes.retain(|r| r.slot != slot);
        }
        Ok(())
    }
}
