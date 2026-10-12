//! Devices in a track's device chain (WP-2.4).
//!
//! A device is a native instrument or effect, or a plug-in, in a track's
//! chain: its kind, its role (MIDI effect, instrument, or audio effect),
//! its names, its activator, and the parameter values the user has set.
//! Each kind of device describes its parameters ([`crate::param`], WP-2.5),
//! which give each value its range.

use imbl::OrdMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{DeviceId, ParamId};

/// A device in a track's chain: a native instrument or effect, or a plug-in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Device {
    /// The device's ID.
    pub id: DeviceId,
    /// The name the title bar shows while Show Preset Name is on: the
    /// device's own name until a preset or Rename gives it another.
    pub name: String,
    /// The device's own name, such as `Auto Filter`, which the title bar
    /// shows while Show Preset Name is off. Empty while it's the same as
    /// [`name`](Self::name), and left out of the saved set then.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub own_name: String,
    /// What the device is.
    pub kind: DeviceKind,
    /// Where it goes in a chain: MIDI effects, then the instrument, then
    /// audio effects. Audio effects aren't marked in the saved set.
    #[serde(default, skip_serializing_if = "DeviceRole::is_audio_effect")]
    pub role: DeviceRole,
    /// Whether the device is switched on. A device that's off passes its
    /// input through unchanged.
    pub enabled: bool,
    /// Whether the title bar shows the preset's or the given name (on, as a
    /// new device is) or the device's own name: the context menu's Show
    /// Preset Name. Left out of the saved set while on.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub show_preset_name: bool,
    /// Parameter values the user has set, in the parameter's own units.
    /// Every value is finite. Parameters not listed keep their defaults.
    ///
    /// They serialise as a list of `[parameter, value]` pairs, since some
    /// formats can't key a map by number.
    #[serde(with = "param_pairs")]
    #[schemars(with = "Vec<(ParamId, f64)>")]
    pub params: OrdMap<ParamId, f64>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_on(value: &bool) -> bool {
    *value
}

fn on() -> bool {
    true
}

impl Device {
    /// A switched-on audio effect with default parameters, named `name`.
    #[must_use]
    pub fn new(id: DeviceId, name: impl Into<String>, kind: DeviceKind) -> Self {
        Self {
            id,
            name: name.into(),
            own_name: String::new(),
            kind,
            role: DeviceRole::AudioEffect,
            enabled: true,
            show_preset_name: true,
            params: OrdMap::new(),
        }
    }

    /// The same device with another role.
    #[must_use]
    pub fn with_role(mut self, role: DeviceRole) -> Self {
        self.role = role;
        self
    }

    /// The device's own name, such as `Auto Filter`, whatever a preset or
    /// Rename called it.
    #[must_use]
    pub fn own_name(&self) -> &str {
        if self.own_name.is_empty() {
            &self.name
        } else {
            &self.own_name
        }
    }

    /// What its title bar shows: its preset's or given name while Show
    /// Preset Name is on, and its own name while it's off.
    #[must_use]
    pub fn title(&self) -> &str {
        if self.show_preset_name {
            &self.name
        } else {
            self.own_name()
        }
    }

    /// Gives the device a new title name (a preset's, or Rename's),
    /// remembering its own name. An empty name gives it its own name back.
    pub(crate) fn set_name(&mut self, name: &str) {
        if self.own_name.is_empty() {
            self.own_name.clone_from(&self.name);
        }
        if name.is_empty() {
            self.name = std::mem::take(&mut self.own_name);
        } else {
            name.clone_into(&mut self.name);
        }
        if self.own_name == self.name {
            self.own_name.clear();
        }
    }
}

/// Where a device goes in a chain. A chain holds its MIDI effects first,
/// then its instrument, then its audio effects, as Live's does; only MIDI
/// tracks hold MIDI effects and instruments, and a chain holds at most one
/// instrument (racks, which hold more, arrive in milestone M5).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DeviceRole {
    /// Takes MIDI and gives MIDI, before the instrument.
    MidiEffect,
    /// Takes MIDI and makes audio.
    Instrument,
    /// Takes audio and gives audio, after the instrument.
    #[default]
    AudioEffect,
}

impl DeviceRole {
    /// Whether it's an audio effect.
    #[must_use]
    pub fn is_audio_effect(&self) -> bool {
        *self == Self::AudioEffect
    }

    /// The name of its kind of device, as the browser's labels and the
    /// hot-swap bar name it: `Audio Effect`, `Instrument`, `MIDI Effect`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::MidiEffect => "MIDI Effect",
            Self::Instrument => "Instrument",
            Self::AudioEffect => "Audio Effect",
        }
    }

    /// Whether a track of `kind` can hold a device with this role: MIDI
    /// effects and instruments only go on MIDI tracks.
    #[must_use]
    pub fn fits(self, kind: crate::TrackKind) -> bool {
        self == Self::AudioEffect || kind == crate::TrackKind::Midi
    }
}

/// What a device is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// A device built into Beatlayer.
    Native {
        /// The device type's stable identifier, such as `beatlayer.utility`.
        device_type: String,
    },
    /// A plug-in.
    Plugin {
        /// The plug-in's format.
        format: PluginFormat,
        /// The plug-in's identifier in its format: a CLAP ID, a VST3 class
        /// ID, an Audio Unit's type, subtype, and manufacturer, or an LV2
        /// URI.
        plugin_id: String,
    },
}

/// A plug-in format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginFormat {
    /// CLAP.
    Clap,
    /// VST3.
    Vst3,
    /// Audio Unit (macOS).
    AudioUnit,
    /// LV2 (Linux).
    Lv2,
}

/// Serialises parameter values as a list of `[parameter, value]` pairs.
pub(crate) mod param_pairs {
    use core::fmt;

    use imbl::OrdMap;
    use serde::de::{Error, SeqAccess, Visitor};
    use serde::ser::SerializeSeq;
    use serde::{Deserializer, Serializer};

    use crate::ids::ParamId;

    pub(crate) fn serialize<S: Serializer>(
        params: &OrdMap<ParamId, f64>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(params.len()))?;
        for pair in params {
            seq.serialize_element(&pair)?;
        }
        seq.end()
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<OrdMap<ParamId, f64>, D::Error> {
        deserializer.deserialize_seq(Pairs)
    }

    struct Pairs;

    impl<'de> Visitor<'de> for Pairs {
        type Value = OrdMap<ParamId, f64>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a list of [parameter, value] pairs")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut params = OrdMap::new();
            while let Some((param, value)) = seq.next_element::<(ParamId, f64)>()? {
                if params.insert(param, value).is_some() {
                    return Err(A::Error::custom(format_args!(
                        "parameter {} is listed twice",
                        param.0
                    )));
                }
            }
            Ok(params)
        }
    }
}
