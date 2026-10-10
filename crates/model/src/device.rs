//! Devices in a track's device chain.
//!
//! These are stubs: enough to place devices in chains, switch them on and
//! off, and store parameter values. WP-2.4 adds device chains to the engine
//! and WP-2.5 the parameter descriptors that give each value its range.

use imbl::OrdMap;
use serde::{Deserialize, Serialize};

use crate::ids::{DeviceId, ParamId};

/// A device in a track's chain: a native instrument or effect, or a plug-in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// The device's ID.
    pub id: DeviceId,
    /// The name shown in the device's title bar, which may be empty.
    pub name: String,
    /// What the device is.
    pub kind: DeviceKind,
    /// Whether the device is switched on. A device that's off passes its
    /// input through unchanged.
    pub enabled: bool,
    /// Parameter values the user has set, in the parameter's own units.
    /// Every value is finite. Parameters not listed keep their defaults.
    ///
    /// They serialise as a list of `[parameter, value]` pairs, since some
    /// formats can't key a map by number.
    #[serde(with = "param_pairs")]
    pub params: OrdMap<ParamId, f64>,
}

impl Device {
    /// A switched-on device with default parameters.
    #[must_use]
    pub fn new(id: DeviceId, name: impl Into<String>, kind: DeviceKind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            enabled: true,
            params: OrdMap::new(),
        }
    }
}

/// What a device is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
mod param_pairs {
    use core::fmt;

    use imbl::OrdMap;
    use serde::de::{Error, SeqAccess, Visitor};
    use serde::ser::SerializeSeq;
    use serde::{Deserializer, Serializer};

    use crate::ids::ParamId;

    pub(super) fn serialize<S: Serializer>(
        params: &OrdMap<ParamId, f64>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(params.len()))?;
        for pair in params {
            seq.serialize_element(&pair)?;
        }
        seq.end()
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
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
