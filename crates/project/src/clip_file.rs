//! Clip files (WP-6.34; the clip files spec, R1 and R2): one clip, with the
//! device chain of the track it was on, in a `.blclip` file of its own.
//!
//! A clip file is one zstd frame with its checksum holding JSON, as a set file
//! is ([`crate::format`]):
//!
//! ```json
//! {"format":"beatlayer-clip","schema":1,"kind":"midi","saved_by":"Beatlayer 0.2.0",
//!  "clip":{ ... },
//!  "devices":[ ... ],
//!  "media":[{"hash":"9f...","path":"Samples/Imported/Kick.wav","size":88244}]}
//! ```
//!
//! `kind` (`audio` or `midi`) comes right after the schema, so the browser can
//! tell the two apart from the first bytes of the frame ([`peek_kind`]). The
//! clip is as a set saves it. `devices` are the track's devices as a rack
//! preset holds them ([`crate::rack_preset`]): racks whole, a plug-in with the
//! state it saved. The audio they play is not in the file: `media` names each
//! file by content hash and size, with its path relative to the clip file's
//! folder. IDs in the file mean nothing: placing it gives everything new ones.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use bl_model::{Clip, ClipData, ContentHash, Device};
use serde::{Deserialize, Serialize};

use crate::error::LoadError;
use crate::media::Media;
use crate::preset::{file_safe, write_whole};

/// A clip file's extension.
pub const CLIP_EXTENSION: &str = "blclip";

/// The `format` field every clip file starts with.
pub const CLIP_FORMAT: &str = "beatlayer-clip";

/// The clip file schema this build writes, and the newest it reads.
pub const CLIP_SCHEMA: u32 = 2;

/// The most JSON a clip file may hold, in bytes (R14): far more than a clip
/// and a chain need, so a small damaged or hostile file can't make the reader
/// decompress without end.
pub const MAX_CLIP_JSON_BYTES: usize = 64 << 20;

/// The largest clip file read, in bytes.
pub const MAX_CLIP_FILE_BYTES: u64 = 64 << 20;

/// Whether a clip file holds an audio clip or a MIDI clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipKind {
    /// An audio clip.
    Audio,
    /// A MIDI clip.
    Midi,
}

impl ClipKind {
    /// The kind of a clip's data.
    #[must_use]
    pub fn of(data: &ClipData) -> Self {
        if data.is_audio() {
            Self::Audio
        } else {
            Self::Midi
        }
    }
}

/// A clip, its track's devices and the files it plays, as a clip file holds
/// them.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipFile {
    /// The clip, as a set saves it.
    pub clip: Clip,
    /// The devices of the clip's track, in chain order, with their state.
    pub devices: Vec<Device>,
    /// Each audio file the clip and the devices play, by path relative to
    /// the clip file's folder.
    pub media: Media,
    /// How long an unwarped audio clip lasted when it was saved, in seconds:
    /// it lasts as long where it lands (R9). `None` for a warped audio clip
    /// and a MIDI clip, whose length is in beats.
    pub seconds: Option<f64>,
}

#[derive(Serialize)]
struct Writing<'a> {
    format: &'static str,
    schema: u32,
    kind: ClipKind,
    saved_by: &'static str,
    clip: &'a Clip,
    devices: &'a [Device],
    media: &'a Media,
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reading {
    #[expect(dead_code, reason = "checked by the probe before this is read")]
    format: String,
    #[expect(dead_code, reason = "checked by the probe before this is read")]
    schema: u32,
    kind: ClipKind,
    #[serde(default)]
    #[expect(dead_code, reason = "tells which Beatlayer wrote it, for people")]
    saved_by: String,
    clip: Clip,
    #[serde(default)]
    devices: Vec<Device>,
    #[serde(default)]
    media: Media,
    #[serde(default)]
    seconds: Option<f64>,
}

/// Why a clip file didn't load or save.
#[derive(Debug)]
#[non_exhaustive]
pub enum ClipFileError {
    /// Reading or writing the file failed.
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        error: io::Error,
    },
    /// The file isn't a Beatlayer clip file, or is damaged.
    Invalid(String),
    /// The file is over the size limit.
    TooLarge,
    /// The file was written by a newer Beatlayer.
    TooNew {
        /// Its schema version.
        found: u32,
    },
}

impl fmt::Display for ClipFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Invalid(why) => write!(f, "not a Beatlayer clip file: {why}"),
            Self::TooLarge => f.write_str("the clip file is larger than a clip file can be"),
            Self::TooNew { found } => write!(
                f,
                "the clip file was saved by a newer Beatlayer (clip file schema {found}; this one \
                 reads up to {CLIP_SCHEMA})"
            ),
        }
    }
}

impl std::error::Error for ClipFileError {}

fn invalid(why: impl Into<String>) -> ClipFileError {
    ClipFileError::Invalid(why.into())
}

impl From<LoadError> for ClipFileError {
    fn from(error: LoadError) -> Self {
        match error {
            LoadError::TooLarge { .. } => Self::TooLarge,
            error => invalid(error.to_string()),
        }
    }
}

impl ClipFile {
    /// The clip file's bytes: the same clip, devices and media always give
    /// the same bytes.
    ///
    /// # Errors
    ///
    /// [`ClipFileError::Invalid`] if the contents can't be written as JSON,
    /// which shouldn't happen for any the model accepts.
    pub fn encode(&self) -> Result<Vec<u8>, ClipFileError> {
        let json = serde_json::to_vec(&Writing {
            format: CLIP_FORMAT,
            schema: CLIP_SCHEMA,
            kind: ClipKind::of(&self.clip.data),
            saved_by: crate::SAVED_BY,
            clip: &self.clip,
            devices: &self.devices,
            media: &self.media,
            seconds: self.seconds,
        })
        .map_err(|e| invalid(e.to_string()))?;
        Ok(crate::format::compress(&json))
    }

    /// Reads a clip file's bytes.
    ///
    /// # Errors
    ///
    /// - [`ClipFileError::TooLarge`] if the JSON is over [`MAX_CLIP_JSON_BYTES`].
    /// - [`ClipFileError::TooNew`] if a newer Beatlayer wrote it.
    /// - [`ClipFileError::Invalid`] if it isn't a clip file, is damaged, or
    ///   breaks a rule: a device ID used twice, a rack in the wrong place, or
    ///   a clip of the other kind than the file says.
    pub fn decode(bytes: &[u8]) -> Result<Self, ClipFileError> {
        let json = crate::format::decompress(bytes, MAX_CLIP_JSON_BYTES)?;
        let value: serde_json::Value =
            serde_json::from_slice(&json).map_err(|e| invalid(e.to_string()))?;
        if value.get("format").and_then(serde_json::Value::as_str) != Some(CLIP_FORMAT) {
            return Err(invalid(format!("its format isn't \"{CLIP_FORMAT}\"")));
        }
        let schema = value
            .get("schema")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| invalid("it has no schema version"))?;
        if schema > u64::from(CLIP_SCHEMA) {
            return Err(ClipFileError::TooNew {
                found: u32::try_from(schema).unwrap_or(u32::MAX),
            });
        }
        // Schema 1 held a note's pressure as points; schema 2 holds it as an
        // envelope of breakpoints (the MPE spec, R2).
        let mut value = value;
        if schema == 1 {
            crate::migrate::pressure_to_breakpoints(&mut value);
        }
        let read: Reading = serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
        if read.kind != ClipKind::of(&read.clip.data) {
            return Err(invalid("its kind isn't the kind of its clip"));
        }
        if read
            .seconds
            .is_some_and(|seconds| !seconds.is_finite() || seconds <= 0.0)
        {
            return Err(invalid("its length in seconds isn't a positive number"));
        }
        let mut seen = BTreeSet::new();
        for id in read.devices.iter().flat_map(Device::ids) {
            if !seen.insert(id) {
                return Err(invalid(format!("the ID {id:?} is used twice")));
            }
        }
        for device in read.devices.iter().flat_map(Device::walk) {
            if let Some(kind) = device.rack_kind()
                && device.role != kind.role()
            {
                return Err(invalid(format!("{} is in the wrong place", device.title())));
            }
        }
        Ok(Self {
            clip: read.clip,
            devices: read.devices,
            media: read.media,
            seconds: read.seconds,
        })
    }

    /// Reads a clip file.
    ///
    /// # Errors
    ///
    /// [`ClipFileError::Io`] if it can't be read, [`ClipFileError::TooLarge`]
    /// if it's over [`MAX_CLIP_FILE_BYTES`], and as [`ClipFile::decode`].
    pub fn load(path: &Path) -> Result<Self, ClipFileError> {
        let io = |error| ClipFileError::Io {
            path: path.to_owned(),
            error,
        };
        if fs::metadata(path).map_err(io)?.len() > MAX_CLIP_FILE_BYTES {
            return Err(ClipFileError::TooLarge);
        }
        Self::decode(&fs::read(path).map_err(io)?)
    }

    /// Writes the clip file to `path`, making its folder if need be, whole or
    /// not at all.
    ///
    /// # Errors
    ///
    /// As [`ClipFile::encode`], and [`ClipFileError::Io`] if it can't be
    /// written.
    pub fn save(&self, path: &Path) -> Result<(), ClipFileError> {
        write_whole(path, &self.encode()?).map_err(|error| match error {
            crate::preset::PresetError::Io { path, error } => ClipFileError::Io { path, error },
            other => invalid(other.to_string()),
        })
    }

    /// The content hashes of every audio file the clip and its devices play.
    #[must_use]
    pub fn media_used(&self) -> BTreeSet<ContentHash> {
        let mut used: BTreeSet<ContentHash> =
            self.devices.iter().flat_map(Device::media_used).collect();
        if let ClipData::Audio(audio) = &self.clip.data {
            used.insert(audio.media);
        }
        used
    }
}

/// Which kind of clip the file at `path` holds, read from the first bytes of
/// its frame without checking the rest; `None` for a file that isn't a clip
/// file. The browser calls this for each clip file it lists.
#[must_use]
pub fn peek_kind(path: &Path) -> Option<ClipKind> {
    // `{"format":"beatlayer-clip","schema":1,"kind":"audio"` is 53 bytes.
    const HEAD: u64 = 128;
    let mut file = fs::File::open(path).ok()?;
    let mut head = Vec::new();
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(&mut file).ok()?;
    (&mut decoder).take(HEAD).read_to_end(&mut head).ok()?;
    let head = String::from_utf8_lossy(&head);
    let rest = head.strip_prefix(&format!("{{\"format\":\"{CLIP_FORMAT}\","))?;
    let (_, rest) = rest.split_once("\"kind\":\"")?;
    match rest.split_once('"')?.0 {
        "audio" => Some(ClipKind::Audio),
        "midi" => Some(ClipKind::Midi),
        _ => None,
    }
}

/// A path in `folder` for a new clip file named `name` that no file has yet:
/// `name.blclip`, or `name 2.blclip`, `name 3.blclip`, and so on (R4).
#[must_use]
pub fn unused_path(folder: &Path, name: &str) -> PathBuf {
    let name = file_safe(name);
    let mut path = folder.join(format!("{name}.{CLIP_EXTENSION}"));
    let mut n = 2u32;
    while path.exists() {
        path = folder.join(format!("{name} {n}.{CLIP_EXTENSION}"));
        n += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use bl_core::id::Id;
    use bl_core::time::Ticks;
    use bl_model::{
        AutomationTarget, Breakpoint, Chain, ChainId, ClipEnvelope, ClipId, DeviceId, DeviceKind,
        MidiData, Note, ParamId, RackKind,
    };

    use super::*;
    use crate::media::{MediaPath, MediaRef};

    fn id(n: u64) -> Id {
        Id::new(n).unwrap()
    }

    const HASH: ContentHash = ContentHash([7; 32]);

    /// An instrument rack holding a plug-in with saved state and a sampler
    /// that plays a sample.
    fn chain() -> Vec<Device> {
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
        let mut sampler = Device::new(
            DeviceId(id(6)),
            "Sampler",
            DeviceKind::Native {
                device_type: "beatlayer.sampler".to_owned(),
            },
        )
        .with_role(bl_model::DeviceRole::Instrument);
        sampler.params.insert(ParamId(2), 0.25);
        sampler.set_media(0, Some(HASH));
        let rack = Device::rack(DeviceId(id(3)), RackKind::Instrument)
            .with_chain(Chain::new(ChainId(id(21)), "Layer").with_devices(vec![sampler, plugin]));
        vec![rack]
    }

    fn midi_clip() -> Clip {
        let notes = [0u8, 60, 127].into_iter().enumerate().map(|(i, key)| {
            let mut note = Note::new(
                key,
                Ticks(i as i64 * 480),
                Ticks(240 + i as i64),
                1 + 63 * i as u8,
            );
            note.release_velocity = 17 * i as u8;
            note.probability = 0.5 + 0.25 * i as f64;
            note.velocity_deviation = -20 * i as i8;
            note.mute = i == 1;
            note.pressure = vec![Breakpoint {
                at: Ticks(10),
                value: 99.0,
                bend: 0.0,
            }];
            note
        });
        let mut data = MidiData::new(notes);
        data.looping = true;
        data.loop_end = Some(Ticks(1920));
        let mut clip = Clip::new(ClipId(id(9)), Ticks(960), Ticks(1920), ClipData::Midi(data));
        clip.name = "Idea".to_owned();
        clip.launch.mode = bl_model::LaunchMode::Gate;
        clip.envelopes.push(ClipEnvelope::new(
            AutomationTarget::Device {
                device: DeviceId(id(6)),
                param: ParamId(2),
            },
            bl_model::Envelope::default(),
        ));
        clip
    }

    fn file() -> ClipFile {
        ClipFile {
            clip: midi_clip(),
            devices: chain(),
            media: Media::from_iter([MediaRef {
                hash: HASH,
                path: MediaPath::new("Samples/Imported/Kick.wav").unwrap(),
                size: 256,
                reversed_from: None,
            }]),
            seconds: None,
        }
    }

    // Spec: clip files R1, R2
    #[test]
    fn a_midi_clip_on_a_track_with_a_rack_round_trips_exactly() {
        let file = file();
        let bytes = file.encode().unwrap();
        assert_eq!(bytes, file.encode().unwrap(), "the same bytes every time");
        assert_eq!(ClipFile::decode(&bytes).unwrap(), file);
    }

    // Spec: clip files R1
    #[test]
    fn an_audio_clips_warp_markers_round_trip() {
        let mut audio = bl_model::AudioData::new(HASH);
        audio.warp = true;
        audio.warp_markers = vec![
            bl_model::WarpMarker {
                time: 0.0,
                beat: Ticks(0),
            },
            bl_model::WarpMarker {
                time: 0.5,
                beat: Ticks(480),
            },
            bl_model::WarpMarker {
                time: 2.0,
                beat: Ticks(1920),
            },
        ];
        audio.looping = true;
        audio.loop_start = 0.5;
        audio.loop_end = Some(2.0);
        let clip = Clip::new(ClipId(id(2)), Ticks(0), Ticks(1920), ClipData::Audio(audio));
        let file = ClipFile {
            clip,
            devices: Vec::new(),
            media: Media::new(),
            seconds: Some(1.5),
        };
        assert_eq!(ClipFile::decode(&file.encode().unwrap()).unwrap(), file);
    }

    // Spec: mpe R2. A clip file of schema 1 holds a note's pressure as
    // points, which load as steps.
    #[test]
    fn a_schema_1_clip_file_loads_its_pressure_points_as_steps() {
        let good = file().encode().unwrap();
        let json = crate::format::decompress(&good, MAX_CLIP_JSON_BYTES).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        value["schema"] = 1.into();
        value["clip"]["data"]["midi"]["notes"][0]["pressure"] =
            serde_json::json!([{ "at": 0, "value": 5 }, { "at": 100, "value": 90 }]);
        let old = crate::format::compress(&serde_json::to_vec(&value).unwrap());
        let loaded = ClipFile::decode(&old).unwrap();
        let ClipData::Midi(midi) = &loaded.clip.data else {
            panic!("a MIDI clip");
        };
        let at = |at: i64, value: f64| Breakpoint {
            at: Ticks(at),
            value,
            bend: 0.0,
        };
        assert_eq!(
            midi.notes[0].pressure,
            [at(0, 5.0), at(99, 5.0), at(100, 90.0)]
        );
    }

    // Spec: clip files R1, R14
    #[test]
    fn newer_and_damaged_clip_files_are_refused() {
        let good = file().encode().unwrap();
        let reframe = |edit: &dyn Fn(&mut serde_json::Value)| {
            let json = crate::format::decompress(&good, MAX_CLIP_JSON_BYTES).unwrap();
            let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
            edit(&mut value);
            crate::format::compress(&serde_json::to_vec(&value).unwrap())
        };
        let newer = reframe(&|v| v["schema"] = (CLIP_SCHEMA + 1).into());
        assert!(matches!(
            ClipFile::decode(&newer),
            Err(ClipFileError::TooNew { found: 3 })
        ));
        let not_a_clip = reframe(&|v| v["format"] = "beatlayer-set".into());
        assert!(matches!(
            ClipFile::decode(&not_a_clip),
            Err(ClipFileError::Invalid(_))
        ));
        let wrong_kind = reframe(&|v| v["kind"] = "audio".into());
        assert!(matches!(
            ClipFile::decode(&wrong_kind),
            Err(ClipFileError::Invalid(_))
        ));
        let twice =
            reframe(&|v| v["devices"][0]["chains"][0]["id"] = v["devices"][0]["id"].clone());
        assert!(matches!(
            ClipFile::decode(&twice),
            Err(ClipFileError::Invalid(_))
        ));
        // Cut short, a flipped byte (the checksum), and not a frame at all.
        assert!(ClipFile::decode(&good[..good.len() / 2]).is_err());
        let mut flipped = good.clone();
        let middle = flipped.len() / 2;
        flipped[middle] ^= 0xff;
        assert!(ClipFile::decode(&flipped).is_err());
        assert!(ClipFile::decode(b"{}").is_err());
        assert!(ClipFile::decode(&[]).is_err());
    }

    // Spec: clip files R14
    #[test]
    fn a_clip_file_over_the_size_limit_is_refused() {
        let json = vec![b' '; MAX_CLIP_JSON_BYTES + 1];
        let bytes = crate::format::compress(&json);
        assert!(matches!(
            ClipFile::decode(&bytes),
            Err(ClipFileError::TooLarge)
        ));
    }

    // Spec: clip files R4, R6
    #[test]
    fn a_clip_file_is_written_whole_named_and_its_kind_peeked() {
        let dir = std::env::temp_dir().join(format!("bl-clip-file-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        let file = file();
        let path = unused_path(&dir, "Idea");
        file.save(&path).unwrap();
        assert_eq!(path.file_name().unwrap(), "Idea.blclip");
        assert_eq!(ClipFile::load(&path).unwrap(), file);
        assert_eq!(peek_kind(&path), Some(ClipKind::Midi));
        assert_eq!(
            unused_path(&dir, "Idea").file_name().unwrap(),
            "Idea 2.blclip"
        );
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "no hidden file left"
        );
        fs::write(dir.join("Other.blclip"), b"not a clip").unwrap();
        assert_eq!(peek_kind(&dir.join("Other.blclip")), None);
        fs::remove_dir_all(&dir).ok();
    }
}
