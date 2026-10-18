//! Rack and chain presets (WP-5.8; the racks spec, R42 and R46): a whole
//! rack, or one chain of a rack, in a file of its own.
//!
//! They are schema 2 of the device preset format ([`crate::preset`]): the
//! same `.blpreset` files and `beatlayer-preset` format, with a `rack` or a
//! `chain` in place of a device's `device` and `params`, so a build that
//! reads only device presets refuses one as newer rather than loading half
//! of it:
//!
//! ```json
//! {"format":"beatlayer-preset","schema":2,"saved_by":"Beatlayer 0.2.0",
//!  "rack":{"id":7,"name":"Audio Effect Rack","kind":{"rack":{"kind":"audio_effect"}},
//!          "chains":[{"id":8,"name":"Wide","devices":[ ... ]}], ...},
//!  "samples":[{"hash":"9f...","name":"Kick.wav","data":"UklGR..."}]}
//! ```
//!
//! The rack is the device as the set holds it, with its chains, their
//! mixers and zones, every device in them with its state and, for a
//! plug-in, the state it saved. A chain preset holds the chain and the kind
//! of rack it came from. The samples the devices play travel inside the file,
//! by content hash, so a preset loads in a set whose media store lacks them.
//! IDs in the file mean nothing: loading gives everything new ones.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use bl_model::{Chain, ContentHash, Device, RackKind};
use serde::{Deserialize, Serialize};

use crate::preset::{MAX_PRESET_BYTES, PRESET_FORMAT, Preset, PresetError, write_whole};

/// The schema rack and chain presets are written in.
pub const RACK_PRESET_SCHEMA: u32 = 2;

/// The largest rack or chain preset file read, in bytes: its samples are in
/// it.
pub const MAX_RACK_PRESET_BYTES: u64 = 256 << 20;

/// An audio file a preset carries: what the device that plays it was loaded
/// from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleFile {
    /// The file's name, with its extension, which tells a decoder what it is.
    pub name: String,
    /// The file's bytes: the content the hash names.
    pub data: Vec<u8>,
}

/// The samples a preset carries, by content hash.
pub type Samples = BTreeMap<ContentHash, SampleFile>;

/// A whole rack, as a preset file holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct RackPreset {
    /// The rack, with its chains and everything in them.
    pub rack: Device,
    /// The samples its devices play.
    pub samples: Samples,
}

/// One chain of a rack, as a preset file holds it (R46).
#[derive(Clone, Debug, PartialEq)]
pub struct ChainPreset {
    /// The kind of rack the chain came from: it loads only into a rack
    /// whose chains hold what this one does.
    pub rack_kind: RackKind,
    /// The chain, with its devices.
    pub chain: Chain,
    /// The samples its devices play.
    pub samples: Samples,
}

/// What a `.blpreset` file holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Saved {
    /// A device's state.
    Device(Preset),
    /// A whole rack.
    Rack(Box<RackPreset>),
    /// One chain.
    Chain(Box<ChainPreset>),
}

#[derive(Serialize, Deserialize)]
struct File {
    format: String,
    schema: u32,
    #[serde(default)]
    saved_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rack: Option<Device>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chain: Option<ChainBody>,
    #[serde(default)]
    samples: Vec<Sample>,
}

#[derive(Serialize, Deserialize)]
struct ChainBody {
    rack_kind: RackKind,
    chain: Chain,
}

#[derive(Serialize, Deserialize)]
struct Sample {
    hash: ContentHash,
    name: String,
    data: String,
}

fn encode(rack: Option<&Device>, chain: Option<ChainBody>, samples: &Samples) -> Vec<u8> {
    let file = File {
        format: PRESET_FORMAT.to_owned(),
        schema: RACK_PRESET_SCHEMA,
        saved_by: crate::SAVED_BY.to_owned(),
        rack: rack.cloned(),
        chain,
        samples: samples
            .iter()
            .map(|(hash, file)| Sample {
                hash: *hash,
                name: file.name.clone(),
                data: bl_model::base64::encode(&file.data),
            })
            .collect(),
    };
    let mut bytes = serde_json::to_vec_pretty(&file).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

fn invalid(why: impl Into<String>) -> PresetError {
    PresetError::Invalid(why.into())
}

/// Checks what a rack or chain preset holds: every ID once, and a sample
/// for every one its devices play.
fn check<'a>(
    devices: impl Iterator<Item = &'a Device>,
    ids: Vec<bl_core::id::Id>,
    used: &std::collections::BTreeSet<ContentHash>,
    samples: &Samples,
) -> Result<(), PresetError> {
    let mut seen = std::collections::BTreeSet::new();
    if let Some(id) = ids.into_iter().find(|id| !seen.insert(*id)) {
        return Err(invalid(format!("the ID {id:?} is used twice")));
    }
    for device in devices {
        if let Some(kind) = device.rack_kind()
            && device.role != kind.role()
        {
            return Err(invalid(format!("{} is in the wrong place", device.title())));
        }
    }
    if let Some(hash) = used.iter().find(|hash| !samples.contains_key(hash)) {
        return Err(invalid(format!("its sample {hash} isn't in the file")));
    }
    Ok(())
}

impl RackPreset {
    /// The preset of `rack` and its `samples`.
    ///
    /// # Errors
    ///
    /// [`PresetError::Invalid`] if the device isn't a rack.
    pub fn new(rack: Device, samples: Samples) -> Result<Self, PresetError> {
        if rack.rack_kind().is_none() {
            return Err(invalid(format!("{} isn't a rack", rack.title())));
        }
        Ok(Self { rack, samples })
    }

    /// The preset file's bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        encode(Some(&self.rack), None, &self.samples)
    }

    /// Writes the preset to `path`, making its folder if need be, whole or
    /// not at all.
    ///
    /// # Errors
    ///
    /// [`PresetError::Io`] if it can't be written.
    pub fn save(&self, path: &Path) -> Result<(), PresetError> {
        write_whole(path, &self.encode())
    }
}

impl ChainPreset {
    /// The preset file's bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let body = ChainBody {
            rack_kind: self.rack_kind,
            chain: self.chain.clone(),
        };
        encode(None, Some(body), &self.samples)
    }

    /// Writes the preset to `path`, making its folder if need be, whole or
    /// not at all.
    ///
    /// # Errors
    ///
    /// [`PresetError::Io`] if it can't be written.
    pub fn save(&self, path: &Path) -> Result<(), PresetError> {
        write_whole(path, &self.encode())
    }
}

impl Saved {
    /// A preset of any kind from a file's bytes.
    ///
    /// # Errors
    ///
    /// [`PresetError::Invalid`] if they aren't a preset or are damaged, and
    /// [`PresetError::TooNew`] if a newer Beatlayer wrote them.
    pub fn decode(bytes: &[u8]) -> Result<Self, PresetError> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        let schema = value.get("schema").and_then(serde_json::Value::as_u64);
        if value.get("rack").is_none() && value.get("chain").is_none() {
            return Preset::decode(bytes).map(Self::Device);
        }
        if value.get("format").and_then(serde_json::Value::as_str) != Some(PRESET_FORMAT) {
            return Err(invalid(format!("its format isn't \"{PRESET_FORMAT}\"")));
        }
        let schema = schema.ok_or_else(|| invalid("it has no schema version"))?;
        if schema > u64::from(RACK_PRESET_SCHEMA) {
            return Err(PresetError::TooNew {
                found: u32::try_from(schema).unwrap_or(u32::MAX),
            });
        }
        if schema < u64::from(RACK_PRESET_SCHEMA) {
            return Err(invalid("a rack or a chain needs schema 2"));
        }
        let file: File = serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
        let mut samples = Samples::new();
        for sample in file.samples {
            let data = bl_model::base64::decode(&sample.data)
                .ok_or_else(|| invalid(format!("the sample {} isn't base64", sample.hash)))?;
            let file = SampleFile {
                name: sample.name,
                data,
            };
            if samples.insert(sample.hash, file).is_some() {
                return Err(invalid(format!(
                    "the sample {} is listed twice",
                    sample.hash
                )));
            }
        }
        match (file.rack, file.chain) {
            (Some(rack), None) => {
                if rack.rack_kind().is_none() {
                    return Err(invalid(format!("{} isn't a rack", rack.title())));
                }
                check(rack.walk(), rack.ids(), &rack.media_used(), &samples)?;
                Ok(Self::Rack(Box::new(RackPreset { rack, samples })))
            }
            (None, Some(ChainBody { rack_kind, chain })) => {
                check(
                    chain.devices_deep(),
                    chain.ids(),
                    &chain.media_used(),
                    &samples,
                )?;
                Ok(Self::Chain(Box::new(ChainPreset {
                    rack_kind,
                    chain,
                    samples,
                })))
            }
            _ => Err(invalid("it holds both a rack and a chain, or neither")),
        }
    }

    /// Reads a preset file of any kind.
    ///
    /// # Errors
    ///
    /// [`PresetError::Io`] if it can't be read, and as [`Saved::decode`].
    pub fn load(path: &Path) -> Result<Self, PresetError> {
        let io = |error| PresetError::Io {
            path: path.to_owned(),
            error,
        };
        let size = fs::metadata(path).map_err(io)?.len();
        if size > MAX_RACK_PRESET_BYTES {
            return Err(invalid(format!(
                "the file is {size} bytes, more than a preset holds"
            )));
        }
        let saved = Self::decode(&fs::read(path).map_err(io)?)?;
        if matches!(saved, Self::Device(_)) && size > MAX_PRESET_BYTES {
            return Err(invalid(format!(
                "the file is {size} bytes, more than a device preset holds"
            )));
        }
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use bl_core::id::Id;
    use bl_model::{ChainId, DeviceId, DeviceKind, ParamId};

    use super::*;

    fn id(n: u64) -> Id {
        Id::new(n).unwrap()
    }

    fn native(n: u64, device_type: &str) -> Device {
        Device::new(
            DeviceId(id(n)),
            device_type,
            DeviceKind::Native {
                device_type: device_type.to_owned(),
            },
        )
    }

    const HASH: ContentHash = ContentHash([7; 32]);

    /// An instrument rack in an audio effect rack, with a plug-in and a
    /// sampler that plays a sample.
    fn nested() -> (Device, Samples) {
        let mut plugin = Device::new(
            DeviceId(id(5)),
            "Reverb",
            DeviceKind::Plugin {
                format: bl_model::PluginFormat::Clap,
                plugin_id: "example.reverb".to_owned(),
            },
        );
        plugin.plugin = Some(bl_model::PluginState {
            path: "/plug-ins/reverb.clap".to_owned(),
            data: vec![0, 1, 2, 254, 255],
        });
        let mut sampler =
            native(6, "beatlayer.sampler").with_role(bl_model::DeviceRole::Instrument);
        sampler.params.insert(ParamId(2), 0.25);
        sampler.set_media(0, Some(HASH));
        let inner = Device::rack(DeviceId(id(4)), RackKind::Instrument)
            .with_chain(Chain::new(ChainId(id(20)), "Keys").with_devices(vec![sampler]));
        let mut outer = Device::rack(DeviceId(id(3)), RackKind::Instrument).with_chain({
            let mut chain = Chain::new(ChainId(id(21)), "Layer").with_devices(vec![inner, plugin]);
            chain.mixer.volume = 0.5;
            chain.colour = Some(12);
            chain
        });
        outer.name = "My Rack".to_owned();
        let samples = Samples::from([(
            HASH,
            SampleFile {
                name: "Kick.wav".to_owned(),
                data: (0..=255).collect(),
            },
        )]);
        (outer, samples)
    }

    // Spec: racks R42
    #[test]
    fn a_rack_preset_round_trips_with_plug_in_state_and_samples() {
        let (rack, samples) = nested();
        let preset = RackPreset::new(rack, samples).unwrap();
        let bytes = preset.encode();
        assert_eq!(
            Saved::decode(&bytes).unwrap(),
            Saved::Rack(Box::new(preset))
        );
    }

    // Spec: racks R46
    #[test]
    fn a_chain_preset_round_trips() {
        let (rack, samples) = nested();
        let preset = ChainPreset {
            rack_kind: RackKind::Instrument,
            chain: rack.chains[0].clone(),
            samples,
        };
        assert_eq!(
            Saved::decode(&preset.encode()).unwrap(),
            Saved::Chain(Box::new(preset))
        );
    }

    // Spec: racks R42
    #[test]
    fn newer_and_damaged_rack_presets_are_refused() {
        let (rack, samples) = nested();
        let good: serde_json::Value =
            serde_json::from_slice(&RackPreset::new(rack, samples).unwrap().encode()).unwrap();
        let refused = |value: &serde_json::Value| {
            Saved::decode(&serde_json::to_vec(value).unwrap()).unwrap_err()
        };
        let mut newer = good.clone();
        newer["schema"] = serde_json::json!(RACK_PRESET_SCHEMA + 1);
        assert!(matches!(refused(&newer), PresetError::TooNew { found: 3 }));
        // A sample the devices play but the file doesn't hold.
        let mut missing = good.clone();
        missing["samples"] = serde_json::json!([]);
        assert!(matches!(refused(&missing), PresetError::Invalid(_)));
        // A rack that isn't one, an ID used twice, text that isn't base64.
        let mut not_rack = good.clone();
        not_rack["rack"] = serde_json::to_value(native(9, "beatlayer.utility")).unwrap();
        assert!(matches!(refused(&not_rack), PresetError::Invalid(_)));
        let mut twice = good.clone();
        twice["rack"]["chains"][0]["id"] = twice["rack"]["id"].clone();
        assert!(matches!(refused(&twice), PresetError::Invalid(_)));
        let mut text = good.clone();
        text["samples"][0]["data"] = serde_json::json!("not base64!");
        assert!(matches!(refused(&text), PresetError::Invalid(_)));
        // Cut short, and a device preset's file with a rack added to it.
        let bytes = RackPreset::new(nested().0, nested().1).unwrap().encode();
        assert!(Saved::decode(&bytes[..bytes.len() / 2]).is_err());
        assert!(Saved::decode(b"{\"format\":\"beatlayer-set\",\"schema\":2,\"rack\":{}}").is_err());
    }

    #[test]
    fn a_device_preset_reads_as_one_and_is_not_a_rack() {
        let mut device = native(1, "beatlayer.utility");
        device.params.insert(ParamId(0), -6.0);
        let preset = Preset::of(&device);
        assert_eq!(
            Saved::decode(&preset.encode()).unwrap(),
            Saved::Device(preset)
        );
        let (rack, samples) = nested();
        assert!(matches!(
            Preset::decode(&RackPreset::new(rack, samples).unwrap().encode()),
            Err(PresetError::Invalid(_))
        ));
        assert!(RackPreset::new(device, Samples::new()).is_err());
    }

    #[test]
    fn a_rack_preset_is_written_whole_or_not_at_all() {
        let dir = std::env::temp_dir().join(format!("bl-rack-preset-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        let (rack, samples) = nested();
        let preset = RackPreset::new(rack, samples).unwrap();
        let path = dir.join("Presets/Instruments/My Rack.blpreset");
        preset.save(&path).unwrap();
        assert_eq!(Saved::load(&path).unwrap(), Saved::Rack(Box::new(preset)));
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        // A folder where the file goes: the write fails and leaves nothing.
        let blocked = dir.join("blocked.blpreset");
        fs::create_dir_all(&blocked).unwrap();
        assert!(matches!(
            RackPreset::new(nested().0, nested().1)
                .unwrap()
                .save(&blocked),
            Err(PresetError::Io { .. })
        ));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(&dir).ok();
    }
}
