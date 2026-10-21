//! Device presets (WP-2.4): a device's state in a file of its own, which
//! the device frame's Save Preset button writes and Hot-Swap Presets loads.
//!
//! A preset file (`.blpreset`) is UTF-8 JSON, Beatlayer's own format:
//!
//! ```json
//! {"format":"beatlayer-preset","schema":1,"saved_by":"Beatlayer 0.2.0",
//!  "device":{"kind":{"native":{"device_type":"beatlayer.utility"}},
//!            "role":"audio_effect","name":"Utility"},
//!  "params":[[0,-6.0],[3,0.5]]}
//! ```
//!
//! `device` says which kind of device the preset is for, so it loads only
//! into a device of that kind; `params` lists every parameter value the
//! device's user had set (any other keeps its default when it loads). The
//! preset's name is its file's name. Plug-ins' own state (WP-2.6) will be
//! a new field, read as absent from schema 1 files.
//!
//! Rack and chain presets are schema 2 of the same format
//! ([`crate::rack_preset`]).
//!
//! Presets live in the User Library: `Presets/<category>/<device name>/`
//! for Save Preset, and `Defaults/<category>/<device name>.blpreset` for Save
//! as Default Preset, where the category is `Audio Effects`, `Instruments`,
//! or `MIDI Effects` (the device chains spec, R17 and R18).

use core::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use bl_model::{Device, DeviceKind, DeviceRole, OrdMap, ParamId};
use serde::{Deserialize, Serialize};

/// A preset file's extension.
pub const PRESET_EXTENSION: &str = "blpreset";

/// The `format` field every preset file starts with.
pub const PRESET_FORMAT: &str = "beatlayer-preset";

/// The preset schema this build writes, and the newest it reads.
pub const PRESET_SCHEMA: u32 = 1;

/// The largest preset file read, in bytes: far more than any parameter list
/// needs, so a stray large file is refused rather than read.
pub const MAX_PRESET_BYTES: u64 = 16 << 20;

/// A device's state, as a preset file holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    /// The kind of device it's for.
    pub kind: DeviceKind,
    /// That device's role.
    pub role: DeviceRole,
    /// That device's own name, such as `Utility`.
    pub device_name: String,
    /// The parameter values set, each finite.
    pub params: OrdMap<ParamId, f64>,
    /// A Shaper's breakpoint shape (the modulation spec, R12, R23);
    /// `None` for the default shape and for any other device. A preset
    /// holds no mappings.
    pub shape: Option<Vec<bl_model::modulator::ShapePoint>>,
    /// An Articulation Map's articulations (the articulation map spec, R16),
    /// in order; `None` for any other device.
    pub articulations: Option<Vec<bl_model::Articulation>>,
}

#[derive(Serialize, Deserialize)]
struct File {
    format: String,
    schema: u32,
    #[serde(default)]
    saved_by: String,
    device: DeviceInfo,
    params: Vec<(ParamId, f64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shape: Option<Vec<bl_model::modulator::ShapePoint>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    articulations: Vec<bl_model::Articulation>,
}

#[derive(Serialize, Deserialize)]
struct DeviceInfo {
    kind: DeviceKind,
    role: DeviceRole,
    name: String,
}

/// Why a preset didn't load or save.
#[derive(Debug)]
#[non_exhaustive]
pub enum PresetError {
    /// Reading or writing the file failed.
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        error: io::Error,
    },
    /// The file isn't a Beatlayer preset, or is damaged.
    Invalid(String),
    /// The file was written by a newer Beatlayer.
    TooNew {
        /// Its schema version.
        found: u32,
    },
    /// The preset is for another kind of device.
    WrongDevice {
        /// The device the preset is for.
        preset: String,
        /// The device it was to load into.
        device: String,
    },
}

impl fmt::Display for PresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Invalid(why) => write!(f, "not a Beatlayer preset: {why}"),
            Self::TooNew { found } => write!(
                f,
                "the preset was saved by a newer Beatlayer (preset schema {found}; this one reads \
                 up to {PRESET_SCHEMA})"
            ),
            Self::WrongDevice { preset, device } => {
                write!(f, "a preset for {preset} can't load into {device}")
            }
        }
    }
}

impl std::error::Error for PresetError {}

impl Preset {
    /// The state of `device`.
    #[must_use]
    pub fn of(device: &Device) -> Self {
        Self {
            kind: device.kind.clone(),
            role: device.role,
            device_name: device.own_name().to_owned(),
            params: device.params.clone(),
            shape: device.shape.clone(),
            articulations: (!device.articulations.is_empty()).then(|| device.articulations.clone()),
        }
    }

    /// Whether the preset loads into `device`: a device of the same kind.
    #[must_use]
    pub fn fits(&self, device: &Device) -> bool {
        self.kind == device.kind
    }

    /// The preset as `device` can load it.
    ///
    /// # Errors
    ///
    /// [`PresetError::WrongDevice`] if it's for another kind of device.
    pub fn check_fits(&self, device: &Device) -> Result<(), PresetError> {
        if self.fits(device) {
            Ok(())
        } else {
            Err(PresetError::WrongDevice {
                preset: self.device_name.clone(),
                device: device.own_name().to_owned(),
            })
        }
    }

    /// The preset file's bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let file = File {
            format: PRESET_FORMAT.to_owned(),
            schema: PRESET_SCHEMA,
            saved_by: crate::SAVED_BY.to_owned(),
            device: DeviceInfo {
                kind: self.kind.clone(),
                role: self.role,
                name: self.device_name.clone(),
            },
            params: self.params.iter().map(|(&p, &v)| (p, v)).collect(),
            shape: self.shape.clone(),
            articulations: self.articulations.clone().unwrap_or_default(),
        };
        let mut bytes = serde_json::to_vec_pretty(&file).unwrap_or_default();
        bytes.push(b'\n');
        bytes
    }

    /// A preset from a file's bytes.
    ///
    /// # Errors
    ///
    /// [`PresetError::Invalid`] if they aren't a preset, and
    /// [`PresetError::TooNew`] if a newer Beatlayer wrote them.
    pub fn decode(bytes: &[u8]) -> Result<Self, PresetError> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| PresetError::Invalid(e.to_string()))?;
        if value.get("rack").is_some() || value.get("chain").is_some() {
            return Err(PresetError::Invalid(
                "it holds a rack or a chain, not a device".to_owned(),
            ));
        }
        if value.get("format").and_then(serde_json::Value::as_str) != Some(PRESET_FORMAT) {
            return Err(PresetError::Invalid(format!(
                "its format isn't \"{PRESET_FORMAT}\""
            )));
        }
        let schema = value
            .get("schema")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| PresetError::Invalid("it has no schema version".to_owned()))?;
        if schema > u64::from(PRESET_SCHEMA) {
            return Err(PresetError::TooNew {
                found: u32::try_from(schema).unwrap_or(u32::MAX),
            });
        }
        let file: File =
            serde_json::from_value(value).map_err(|e| PresetError::Invalid(e.to_string()))?;
        let mut params = OrdMap::new();
        for (param, value) in file.params {
            if !value.is_finite() {
                return Err(PresetError::Invalid(format!(
                    "parameter {} has the value {value}",
                    param.0
                )));
            }
            if params.insert(param, value).is_some() {
                return Err(PresetError::Invalid(format!(
                    "parameter {} is listed twice",
                    param.0
                )));
            }
        }
        // A preset saved before T197 names the device as Beatlayer then did.
        let mut device_name = file.device.name;
        if let DeviceKind::Native { device_type } = &file.device.kind
            && let Some(now) = crate::migrate::renamed_device(device_type, &device_name)
        {
            device_name = now.to_owned();
        }
        // Spec: articulation-map R20. A preset whose list breaks the rules
        // doesn't load.
        if !file.articulations.is_empty()
            && let Err(why) = bl_model::articulation::check(&file.articulations)
        {
            return Err(PresetError::Invalid(why.to_owned()));
        }
        Ok(Self {
            kind: file.device.kind,
            role: file.device.role,
            device_name,
            params,
            shape: file.shape,
            articulations: (!file.articulations.is_empty()).then_some(file.articulations),
        })
    }

    /// Reads a preset file.
    ///
    /// # Errors
    ///
    /// [`PresetError::Io`] if it can't be read, and as [`Preset::decode`].
    pub fn load(path: &Path) -> Result<Self, PresetError> {
        let io = |error| PresetError::Io {
            path: path.to_owned(),
            error,
        };
        let size = fs::metadata(path).map_err(io)?.len();
        if size > MAX_PRESET_BYTES {
            return Err(PresetError::Invalid(format!(
                "the file is {size} bytes, more than a preset holds"
            )));
        }
        Self::decode(&fs::read(path).map_err(io)?)
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

/// Writes `bytes` to `path`, making its folder if need be. The file is
/// written beside it under a hidden name and renamed into place, so `path`
/// holds a whole file, old or new, whenever the write stops.
pub fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), PresetError> {
    let io = |error| PresetError::Io {
        path: path.to_owned(),
        error,
    };
    let folder = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(folder).map_err(io)?;
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let hidden = folder.join(format!(".{name}.saving"));
    let written = (|| {
        let mut file = fs::File::create(&hidden)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&hidden, path)
    })();
    if written.is_err() {
        fs::remove_file(&hidden).ok();
    }
    written.map_err(io)
}

/// The preset's name: its file's name without the extension.
#[must_use]
pub fn preset_name(path: &Path) -> String {
    path.file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
}

/// The User Library's folder for presets of a role: `Audio Effects`,
/// `Instruments`, or `MIDI Effects`, as Live's are named.
#[must_use]
pub fn category(role: DeviceRole) -> &'static str {
    match role {
        DeviceRole::MidiEffect => "MIDI Effects",
        DeviceRole::Instrument => "Instruments",
        DeviceRole::AudioEffect => "Audio Effects",
    }
}

/// A file name made safe on every platform: path separators and the
/// characters Windows refuses become `-`, and an empty name `Untitled`.
#[must_use]
pub fn file_safe(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let safe = safe.trim().trim_start_matches('.').trim_end_matches('.');
    if safe.is_empty() {
        "Untitled".to_owned()
    } else {
        safe.to_owned()
    }
}

/// Where Save Preset puts presets for `device`:
/// `<User Library>/Presets/<category>/<device's own name>`.
#[must_use]
pub fn presets_folder(user_library: &Path, device: &Device) -> PathBuf {
    user_library
        .join("Presets")
        .join(category(device.role))
        .join(file_safe(device.own_name()))
}

/// Where Save as Default Preset puts `device`'s default:
/// `<User Library>/Defaults/<category>/<device's own name>.blpreset`.
#[must_use]
pub fn default_preset_path(user_library: &Path, device: &Device) -> PathBuf {
    default_path(user_library, device.role, device.own_name())
}

/// The default preset's path for a device with this role and own name.
#[must_use]
pub fn default_path(user_library: &Path, role: DeviceRole, own_name: &str) -> PathBuf {
    user_library
        .join("Defaults")
        .join(category(role))
        .join(format!("{}.{PRESET_EXTENSION}", file_safe(own_name)))
}

/// A path in `folder` for a new preset named `name` that no file has yet:
/// `name.blpreset`, or `name 2.blpreset`, `name 3.blpreset`, and so on.
#[must_use]
pub fn unused_path(folder: &Path, name: &str) -> PathBuf {
    let name = file_safe(name);
    let mut path = folder.join(format!("{name}.{PRESET_EXTENSION}"));
    let mut n = 2u32;
    while path.exists() {
        path = folder.join(format!("{name} {n}.{PRESET_EXTENSION}"));
        n += 1;
    }
    path
}

/// Whether a path names a preset file, by its extension.
#[must_use]
pub fn is_preset(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(PRESET_EXTENSION))
}

#[cfg(test)]
mod tests {
    use bl_core::id::Id;
    use bl_model::DeviceId;

    use super::*;

    fn device() -> Device {
        let mut device = Device::new(
            DeviceId(Id::new(7).unwrap()),
            "Utility",
            DeviceKind::Native {
                device_type: "beatlayer.utility".to_owned(),
            },
        );
        device.params.insert(ParamId(0), -6.0);
        device.params.insert(ParamId(3), 1.0 / 3.0);
        device
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bl-preset-{name}-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        dir
    }

    // Spec: device-chains R16
    #[test]
    fn a_preset_round_trips_exactly() {
        let preset = Preset::of(&device());
        let bytes = preset.encode();
        assert_eq!(Preset::decode(&bytes).unwrap(), preset);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("\"format\": \"beatlayer-preset\""), "{text}");
    }

    // Spec: device-chains R16
    #[test]
    fn a_preset_saved_under_a_devices_old_name_loads_under_its_new_one() {
        let old = Device::new(
            DeviceId(Id::new(8).unwrap()),
            "Squeeze",
            DeviceKind::Native {
                device_type: "beatlayer.squeeze".to_owned(),
            },
        );
        let loaded = Preset::decode(&Preset::of(&old).encode()).unwrap();
        assert_eq!(loaded.device_name, "Compressor");
        assert_eq!(loaded.kind, old.kind);
    }

    #[test]
    fn foreign_newer_and_damaged_files_are_refused() {
        assert!(matches!(
            Preset::decode(b"{\"format\":\"beatlayer-set\",\"schema\":1}"),
            Err(PresetError::Invalid(_))
        ));
        let mut value: serde_json::Value =
            serde_json::from_slice(&Preset::of(&device()).encode()).unwrap();
        value["schema"] = serde_json::json!(PRESET_SCHEMA + 1);
        assert!(matches!(
            Preset::decode(&serde_json::to_vec(&value).unwrap()),
            Err(PresetError::TooNew { .. })
        ));
        value["schema"] = serde_json::json!(1);
        value["params"] = serde_json::json!([[1, 0.5], [1, 0.25]]);
        assert!(matches!(
            Preset::decode(&serde_json::to_vec(&value).unwrap()),
            Err(PresetError::Invalid(_))
        ));
        assert!(Preset::decode(b"\xff").is_err());
    }

    #[test]
    fn a_preset_fits_only_its_kind_of_device() {
        let preset = Preset::of(&device());
        assert!(preset.check_fits(&device()).is_ok());
        let other = Device::new(
            DeviceId(Id::new(8).unwrap()),
            "Echo",
            DeviceKind::Native {
                device_type: "beatlayer.echo".to_owned(),
            },
        );
        assert_eq!(
            preset.check_fits(&other).unwrap_err().to_string(),
            "a preset for Utility can't load into Echo"
        );
    }

    // Spec: device-chains R17, R18
    #[test]
    fn presets_go_in_the_user_library_under_new_names() {
        let library = scratch("library");
        let device = device();
        let folder = presets_folder(&library, &device);
        assert!(folder.ends_with("Presets/Audio Effects/Utility"));
        let first = unused_path(&folder, "Utility");
        Preset::of(&device).save(&first).unwrap();
        assert_eq!(first.file_name().unwrap(), "Utility.blpreset");
        let second = unused_path(&folder, "Utility");
        assert_eq!(second.file_name().unwrap(), "Utility 2.blpreset");
        assert_eq!(Preset::load(&first).unwrap(), Preset::of(&device));
        assert_eq!(preset_name(&second), "Utility 2");
        assert!(
            default_preset_path(&library, &device)
                .ends_with("Defaults/Audio Effects/Utility.blpreset")
        );
        // Nothing hidden is left behind.
        let names: Vec<_> = fs::read_dir(&folder).unwrap().flatten().collect();
        assert_eq!(names.len(), 1);
        assert_eq!(file_safe("A/B: C?"), "A-B- C-");
        assert_eq!(file_safe(".."), "Untitled");
        fs::remove_dir_all(&library).ok();
    }
}
