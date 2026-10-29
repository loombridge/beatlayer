//! The MPE/Multi-channel Settings and a plug-in's MPE Mode (WP-6.17b; the MPE
//! spec, R14, R17 and R18).
//!
//! MPE MIDI goes out in one zone of a MIDI port or a plug-in: a master
//! channel for the messages that aren't a note's and member channels for the
//! notes, or in multi-channel mode on any range of channels. The same
//! settings serve a plug-in device, a MIDI track's MIDI To and an External
//! Instrument's.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ModelError;

/// The most note channels a zone has: every channel but its master (R18).
pub const MAX_NOTE_CHANNELS: u8 = 15;

/// Which end of the channels a zone's master channel is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MpeZone {
    /// Master channel 1, note channels 2 and up.
    #[default]
    Lower,
    /// Master channel 16, note channels 15 and down.
    Upper,
}

/// Whether the notes go out in a zone, or on a range of channels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MpeMode {
    /// One zone of master and note channels.
    #[default]
    Zone,
    /// Multi-channel mode: notes on the channels of a range, with no master.
    MultiChannel,
}

/// The MPE/Multi-channel Settings (the MPE spec, R18).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    description = "MPE/Multi-channel Settings: `mode` (`zone` or `multi_channel`), `zone` \
                   (`lower` with master channel 1, or `upper` with 16) and `channels` (its note \
                   channels, 1 to 15) for a zone; `first` and `last` (1 to 16) for \
                   multi-channel mode. Anything left out takes the new settings' value: the \
                   lower zone with 15 note channels."
)]
pub struct MpeSettings {
    /// Zone or multi-channel mode.
    #[serde(default, skip_serializing_if = "is_default")]
    pub mode: MpeMode,
    /// The zone, in zone mode.
    #[serde(default, skip_serializing_if = "is_default")]
    pub zone: MpeZone,
    /// The zone's number of note channels, 1 to 15.
    #[serde(default = "fifteen", skip_serializing_if = "is_fifteen")]
    pub channels: u8,
    /// Multi-channel mode's first channel, 1 to 16.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub first: u8,
    /// Its last channel, 1 to 16, not below the first.
    #[serde(default = "sixteen", skip_serializing_if = "is_sixteen")]
    pub last: u8,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn is_fifteen(value: &u8) -> bool {
    *value == MAX_NOTE_CHANNELS
}

fn is_one(value: &u8) -> bool {
    *value == 1
}

fn is_sixteen(value: &u8) -> bool {
    *value == 16
}

fn fifteen() -> u8 {
    MAX_NOTE_CHANNELS
}

fn one() -> u8 {
    1
}

fn sixteen() -> u8 {
    16
}

impl Default for MpeSettings {
    /// New settings: the lower zone with 15 note channels (R18).
    fn default() -> Self {
        Self {
            mode: MpeMode::Zone,
            zone: MpeZone::Lower,
            channels: MAX_NOTE_CHANNELS,
            first: 1,
            last: 16,
        }
    }
}

impl MpeSettings {
    /// Checks the numbers the mode uses.
    ///
    /// # Errors
    ///
    /// [`ModelError::InvalidInput`] when a zone's note channels aren't 1 to
    /// 15, or multi-channel mode's range isn't inside 1 to 16 or ends before
    /// it starts.
    pub fn check(&self) -> Result<(), ModelError> {
        match self.mode {
            MpeMode::Zone if !(1..=MAX_NOTE_CHANNELS).contains(&self.channels) => {
                Err(ModelError::InvalidInput("a zone has 1 to 15 note channels"))
            }
            MpeMode::MultiChannel
                if !(1..=16).contains(&self.first)
                    || !(1..=16).contains(&self.last)
                    || self.first > self.last =>
            {
                Err(ModelError::InvalidInput(
                    "multi-channel mode's channels run from 1 to 16, the first not above the last",
                ))
            }
            _ => Ok(()),
        }
    }

    /// Whether these are the new settings.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A plug-in device's MPE Mode, its settings and its tuning switch (the MPE
/// spec, R14, R18; the tunings spec, R20).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    description = "A plug-in device's MPE: `on` (MPE Mode, off in a new device), `settings` \
                   (the MPE/Multi-channel Settings; left out while they are the new settings') \
                   and `tuning` (Apply Tuning to Per-Note Pitch Bend, on in a new device; left \
                   out while on)."
)]
pub struct PluginMpe {
    /// MPE Mode.
    pub on: bool,
    /// Apply Tuning to Per-Note Pitch Bend (the tunings spec, R20): on in a
    /// new device, so saved only when off. It reaches the plug-in only
    /// while MPE Mode is on.
    #[serde(default = "tuning_on", skip_serializing_if = "is_tuning_on")]
    pub tuning: bool,
    /// The settings MPE MIDI goes out on, left out while they are the new
    /// settings'.
    #[serde(default, skip_serializing_if = "MpeSettings::is_default")]
    pub settings: MpeSettings,
}

const fn tuning_on() -> bool {
    true
}

/// For serde's `skip_serializing_if`, which passes a reference.
fn is_tuning_on(tuning: &bool) -> bool {
    *tuning
}

impl Default for PluginMpe {
    fn default() -> Self {
        Self {
            on: false,
            tuning: true,
            settings: MpeSettings::default(),
        }
    }
}

impl PluginMpe {
    /// Whether it is a new device's: off, with the new settings and the
    /// tuning switch on.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: mpe R18
    #[test]
    fn new_settings_are_the_lower_zone_with_fifteen_note_channels() {
        let settings: MpeSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, MpeSettings::default());
        assert_eq!(settings.mode, MpeMode::Zone);
        assert_eq!(settings.zone, MpeZone::Lower);
        assert_eq!(settings.channels, 15);
        assert!(settings.check().is_ok());
    }

    // Spec: mpe R18
    #[test]
    fn settings_outside_their_ranges_are_refused() {
        let zone = |channels| MpeSettings {
            channels,
            ..MpeSettings::default()
        };
        assert!(zone(0).check().is_err());
        assert!(zone(16).check().is_err());
        assert!(zone(1).check().is_ok());
        let multi = |first, last| MpeSettings {
            mode: MpeMode::MultiChannel,
            first,
            last,
            ..MpeSettings::default()
        };
        assert!(multi(0, 4).check().is_err());
        assert!(multi(3, 17).check().is_err());
        assert!(multi(5, 4).check().is_err());
        assert!(multi(2, 2).check().is_ok());
        assert!(multi(1, 16).check().is_ok());
    }

    // Spec: mpe R14
    #[test]
    fn a_new_devices_mpe_is_left_out_of_the_saved_set() {
        assert!(PluginMpe::default().is_default());
        assert_eq!(
            serde_json::to_value(PluginMpe::default()).unwrap(),
            serde_json::json!({"on": false})
        );
    }
}
