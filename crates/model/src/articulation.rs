//! The articulations of an Articulation Map (WP-6.47; the articulation map
//! spec): the list a device holds, each articulation's trigger, outputs and
//! channel, and the rules that keep the list sound.
//!
//! The list lives on the device, in the document, and is saved with the set
//! and in the device's presets. The engine reads it as [`List`], a fixed
//! size made with the node, so a change reaches the playing device without a
//! new program (the spec, R14).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Document;
use crate::error::ModelError;
use crate::ids::DeviceId;

/// The Articulation Map's device type.
pub const ARTICULATION_MAP: &str = "beatlayer.articulation_map";

/// The most articulations a device holds.
pub const MAX_ARTICULATIONS: usize = 32;

/// The most outputs one articulation has.
pub const MAX_OUTPUTS: usize = 4;

/// The name a new device's first articulation has.
pub const FIRST_NAME: &str = "Articulation 1";

/// What makes an articulation current.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ArticulationTrigger {
    /// None: only the Articulation control chooses it.
    None,
    /// A key, 0 to 127, matched on any channel at any velocity.
    Key {
        /// The key.
        key: u8,
    },
    /// A controller, 0 to 127, with a range of values from a lowest to a
    /// highest.
    Controller {
        /// The controller.
        controller: u8,
        /// The lowest value of the range.
        lowest: u8,
        /// The highest value of the range.
        highest: u8,
    },
}

impl ArticulationTrigger {
    /// The key this trigger matches, if it matches a key.
    #[must_use]
    pub fn key(&self) -> Option<u8> {
        match self {
            Self::Key { key } => Some(*key),
            _ => None,
        }
    }

    /// The controller and the range of values this trigger matches, if it
    /// matches a controller.
    #[must_use]
    pub fn controller(&self) -> Option<(u8, u8, u8)> {
        match self {
            Self::Controller {
                controller,
                lowest,
                highest,
            } => Some((*controller, *lowest, *highest)),
            _ => None,
        }
    }

    /// Whether a note on `key` is this trigger.
    #[must_use]
    pub fn matches_key(&self, key: u8) -> bool {
        self.key() == Some(key)
    }

    /// Whether a control change of `controller` to `value` is this trigger.
    #[must_use]
    pub fn matches_control(&self, controller: u8, value: u8) -> bool {
        self.controller().is_some_and(|(c, lowest, highest)| {
            c == controller && value >= lowest && value <= highest
        })
    }
}

/// What an articulation sends when it becomes current.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ArticulationOutput {
    /// A key, 0 to 127, with a velocity, 1 to 127.
    Key {
        /// The key.
        key: u8,
        /// The velocity.
        velocity: u8,
    },
    /// A controller, 0 to 127, with a value, 0 to 127.
    Controller {
        /// The controller.
        controller: u8,
        /// The value.
        value: u8,
    },
}

/// One articulation of an Articulation Map.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Articulation {
    /// Its name, which the panel shows and the Articulation control reads.
    pub name: String,
    /// What makes it current.
    pub trigger: ArticulationTrigger,
    /// What it sends when it becomes current, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<ArticulationOutput>,
    /// The channel its notes go out on, or `None` for the channel they come
    /// in on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
}

impl Articulation {
    /// An articulation with a name and nothing else: no trigger, no outputs
    /// and no channel.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            trigger: ArticulationTrigger::None,
            outputs: Vec::new(),
            channel: None,
        }
    }
}

/// An articulation as the audio thread holds it: its trigger, its outputs in
/// a fixed-size array, and its channel. Its name is the panel's, and the
/// engine never needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// What makes it current.
    pub trigger: ArticulationTrigger,
    /// What it sends when it becomes current, in order; only the first
    /// `output_count` of them.
    pub outputs: [ArticulationOutput; MAX_OUTPUTS],
    /// How many of the outputs it has.
    pub output_count: u8,
    /// The channel its notes go out on, or 0 for the channel they come in on.
    pub channel: u8,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            trigger: ArticulationTrigger::None,
            outputs: [ArticulationOutput::Controller {
                controller: 0,
                value: 0,
            }; MAX_OUTPUTS],
            output_count: 0,
            channel: 0,
        }
    }
}

/// The articulations of a device as the audio thread holds them: a fixed
/// size, made with the node, so nothing on the audio thread allocates
/// (the spec, R14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct List {
    /// How many articulations the list holds.
    pub count: u8,
    /// The articulations, in order; only the first `count` of them.
    pub entries: [Entry; MAX_ARTICULATIONS],
}

impl Default for List {
    fn default() -> Self {
        Self {
            count: 0,
            entries: [Entry::default(); MAX_ARTICULATIONS],
        }
    }
}

impl List {
    /// The list of `articulations`, as the audio thread holds it. A list
    /// longer than [`MAX_ARTICULATIONS`] is cut short.
    #[must_use]
    pub fn of(articulations: &[Articulation]) -> Self {
        let mut list = Self::default();
        for articulation in articulations.iter().take(MAX_ARTICULATIONS) {
            let mut entry = Entry {
                trigger: articulation.trigger,
                output_count: articulation.outputs.len().min(MAX_OUTPUTS) as u8,
                channel: articulation.channel.unwrap_or(0),
                ..Entry::default()
            };
            for (slot, output) in articulation.outputs.iter().take(MAX_OUTPUTS).enumerate() {
                entry.outputs[slot] = *output;
            }
            list.entries[usize::from(list.count)] = entry;
            list.count += 1;
        }
        list
    }

    /// The articulation at `index`, if the list has one.
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<&Entry> {
        (index < usize::from(self.count)).then(|| &self.entries[index])
    }

    /// How many articulations the list holds.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.count)
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Why a list of articulations is refused, if it is.
pub fn check(articulations: &[Articulation]) -> Result<(), &'static str> {
    if articulations.is_empty() {
        return Err("the articulation map's list holds no articulation");
    }
    if articulations.len() > MAX_ARTICULATIONS {
        return Err("the articulation map's list holds more than 32 articulations");
    }
    for articulation in articulations {
        check_one(articulation)?;
    }
    // Spec: articulation-map R3. No two articulations have the same key, and
    // no two have overlapping ranges of the same controller.
    for (index, articulation) in articulations.iter().enumerate() {
        for other in &articulations[index + 1..] {
            if let (Some(key), Some(other_key)) = (articulation.trigger.key(), other.trigger.key())
                && key == other_key
            {
                return Err("two articulations trigger on the same key");
            }
            if let (
                Some((controller, lowest, highest)),
                Some((other_controller, other_lowest, other_highest)),
            ) = (
                articulation.trigger.controller(),
                other.trigger.controller(),
            ) && controller == other_controller
                && lowest <= other_highest
                && other_lowest <= highest
            {
                return Err("two articulations trigger on the same controller");
            }
        }
    }
    Ok(())
}

/// Checks one articulation's own values.
fn check_one(articulation: &Articulation) -> Result<(), &'static str> {
    match articulation.trigger {
        ArticulationTrigger::None => {}
        ArticulationTrigger::Key { key } => {
            if key > 127 {
                return Err("an articulation's trigger key is outside 0 to 127");
            }
        }
        ArticulationTrigger::Controller {
            controller,
            lowest,
            highest,
        } => {
            if controller > 127 {
                return Err("an articulation's trigger controller is outside 0 to 127");
            }
            if lowest > 127 || highest > 127 {
                return Err("an articulation's trigger value is outside 0 to 127");
            }
            if lowest > highest {
                return Err("an articulation's trigger range is empty");
            }
        }
    }
    if articulation.outputs.len() > MAX_OUTPUTS {
        return Err("an articulation has more than four outputs");
    }
    for output in &articulation.outputs {
        match *output {
            ArticulationOutput::Key { key, velocity } => {
                if key > 127 {
                    return Err("an articulation's output key is outside 0 to 127");
                }
                if !(1..=127).contains(&velocity) {
                    return Err("an articulation's output velocity is outside 1 to 127");
                }
            }
            ArticulationOutput::Controller { controller, value } => {
                if controller > 127 {
                    return Err("an articulation's output controller is outside 0 to 127");
                }
                if value > 127 {
                    return Err("an articulation's output value is outside 0 to 127");
                }
            }
        }
    }
    if let Some(channel) = articulation.channel
        && !(1..=16).contains(&channel)
    {
        return Err("an articulation's channel is outside 1 to 16");
    }
    Ok(())
}

/// The name of the articulation at `index`, or a numbered one when it has
/// none.
#[must_use]
pub fn name_at(articulations: &[Articulation], index: usize) -> String {
    articulations
        .get(index)
        .map_or_else(|| format!("Articulation {}", index + 1), |a| a.name.clone())
}

/// Reads a value that may be given as `null`: `Some(None)` for a null, and
/// `None` for a value that isn't given at all.
pub fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

impl Document {
    /// Adds an articulation to the Articulation Map `device`'s list, at
    /// `index` or at the end (the articulation map spec, R16).
    pub(crate) fn add_articulation(
        &mut self,
        device: DeviceId,
        index: Option<u8>,
        name: &str,
    ) -> Result<(), ModelError> {
        let mut list = self.articulation_list(device)?;
        if list.len() >= MAX_ARTICULATIONS {
            return Err(ModelError::InvalidInput(
                "the articulation map's list holds 32 articulations, more than it takes",
            ));
        }
        let at = match index {
            Some(at) => usize::from(at),
            None => list.len(),
        };
        if at > list.len() {
            return Err(ModelError::InvalidInput(
                "there is no articulation at that place in the list",
            ));
        }
        let name = if name.is_empty() {
            format!("Articulation {}", list.len() + 1)
        } else {
            name.to_owned()
        };
        list.insert(at, Articulation::new(name));
        self.set_articulation_list(device, list)
    }

    /// Removes the articulation at `index` from the Articulation Map
    /// `device`'s list (the articulation map spec, R16). The list keeps at
    /// least one articulation.
    pub(crate) fn delete_articulation(
        &mut self,
        device: DeviceId,
        index: u8,
    ) -> Result<(), ModelError> {
        let mut list = self.articulation_list(device)?;
        if list.len() == 1 {
            return Err(ModelError::InvalidInput(
                "the articulation map's list keeps one articulation",
            ));
        }
        let at = usize::from(index);
        if at >= list.len() {
            return Err(ModelError::InvalidInput(
                "there is no articulation at that place in the list",
            ));
        }
        list.remove(at);
        self.set_articulation_list(device, list)
    }

    /// Moves the articulation at `index` in the Articulation Map `device`'s
    /// list to `to`, or to the end (the articulation map spec, R16).
    pub(crate) fn move_articulation(
        &mut self,
        device: DeviceId,
        index: u8,
        to: Option<u8>,
    ) -> Result<(), ModelError> {
        let mut list = self.articulation_list(device)?;
        let at = usize::from(index);
        if at >= list.len() {
            return Err(ModelError::InvalidInput(
                "there is no articulation at that place in the list",
            ));
        }
        let last = list.len() - 1;
        let place = match to {
            Some(place) => usize::from(place),
            None => last,
        };
        if place > last {
            return Err(ModelError::InvalidInput(
                "there is no articulation at that place in the list",
            ));
        }
        let articulation = list.remove(at);
        list.insert(place, articulation);
        self.set_articulation_list(device, list)
    }

    /// Changes the articulation at `index` in the Articulation Map `device`'s
    /// list: its name, its trigger, its outputs or its channel (the
    /// articulation map spec, R16). What isn't given stays as it is.
    pub(crate) fn set_articulation(
        &mut self,
        device: DeviceId,
        index: u8,
        name: Option<&str>,
        trigger: Option<ArticulationTrigger>,
        outputs: Option<&[ArticulationOutput]>,
        channel: Option<Option<u8>>,
    ) -> Result<(), ModelError> {
        let mut list = self.articulation_list(device)?;
        let at = usize::from(index);
        if at >= list.len() {
            return Err(ModelError::InvalidInput(
                "there is no articulation at that place in the list",
            ));
        }
        if let Some(name) = name {
            list[at].name = name.to_owned();
        }
        if let Some(trigger) = trigger {
            list[at].trigger = trigger;
        }
        if let Some(outputs) = outputs {
            list[at].outputs = outputs.to_vec();
        }
        if let Some(channel) = channel {
            list[at].channel = channel;
        }
        self.set_articulation_list(device, list)
    }

    /// The articulations the Articulation Map `device` holds.
    fn articulation_list(&self, device: DeviceId) -> Result<Vec<Articulation>, ModelError> {
        let device = self.device_ref(device)?;
        if !device.is_articulation_map() {
            return Err(ModelError::InvalidInput(
                "only an Articulation Map holds articulations",
            ));
        }
        Ok(device.articulations.clone())
    }

    /// Puts `list` on the Articulation Map `device`, once the list is sound.
    fn set_articulation_list(
        &mut self,
        device: DeviceId,
        list: Vec<Articulation>,
    ) -> Result<(), ModelError> {
        check(&list).map_err(ModelError::InvalidInput)?;
        self.device_mut(device)?.set_articulations(list);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn articulation(name: &str) -> Articulation {
        Articulation::new(name)
    }

    // Spec: articulation-map R2, R3.
    #[test]
    fn a_list_of_one_articulation_is_sound() {
        let list = vec![articulation("Staccato")];
        check(&list).unwrap();
    }

    // Spec: articulation-map R2, R20.
    #[test]
    fn an_empty_or_long_list_is_refused() {
        assert!(check(&[]).is_err());
        let long: Vec<Articulation> = (0..=MAX_ARTICULATIONS)
            .map(|i| articulation(&format!("A {i}")))
            .collect();
        assert!(check(&long).is_err());
    }

    // Spec: articulation-map R3.
    #[test]
    fn two_articulations_with_the_same_key_are_refused() {
        let mut list = vec![articulation("A"), articulation("B")];
        list[0].trigger = ArticulationTrigger::Key { key: 36 };
        list[1].trigger = ArticulationTrigger::Key { key: 36 };
        assert!(check(&list).is_err());
        list[1].trigger = ArticulationTrigger::Key { key: 37 };
        check(&list).unwrap();
    }

    // Spec: articulation-map R3.
    #[test]
    fn two_articulations_with_overlapping_controller_ranges_are_refused() {
        let mut list = vec![articulation("A"), articulation("B")];
        list[0].trigger = ArticulationTrigger::Controller {
            controller: 1,
            lowest: 0,
            highest: 63,
        };
        list[1].trigger = ArticulationTrigger::Controller {
            controller: 1,
            lowest: 32,
            highest: 64,
        };
        assert!(check(&list).is_err());
        list[1].trigger = ArticulationTrigger::Controller {
            controller: 1,
            lowest: 64,
            highest: 96,
        };
        check(&list).unwrap();
    }

    // Spec: articulation-map R4, R5, R20.
    #[test]
    fn values_outside_their_ranges_are_refused() {
        let mut list = vec![articulation("A")];
        list[0].outputs = vec![ArticulationOutput::Key {
            key: 200,
            velocity: 100,
        }];
        assert!(check(&list).is_err());
        list[0].outputs = vec![ArticulationOutput::Key {
            key: 36,
            velocity: 0,
        }];
        assert!(check(&list).is_err());
        list[0].outputs = vec![ArticulationOutput::Key {
            key: 36,
            velocity: 100,
        }];
        check(&list).unwrap();
        list[0].channel = Some(17);
        assert!(check(&list).is_err());
        list[0].channel = Some(16);
        check(&list).unwrap();
    }

    // Spec: articulation-map R4, R14.
    #[test]
    fn the_list_the_engine_holds_keeps_the_order_and_the_own_data() {
        let mut list = vec![articulation("A"), articulation("B")];
        list[1].trigger = ArticulationTrigger::Key { key: 36 };
        list[1].outputs = vec![ArticulationOutput::Controller {
            controller: 7,
            value: 100,
        }];
        list[1].channel = Some(5);
        let held = List::of(&list);
        assert_eq!(held.len(), 2);
        let second = held.entry(1).unwrap();
        assert_eq!(second.trigger, ArticulationTrigger::Key { key: 36 });
        assert_eq!(second.output_count, 1);
        assert_eq!(second.channel, 5);
        assert!(held.entry(2).is_none());
    }
}
