//! Groove files (WP-6.19, the grooves spec R4): a groove in a file of its
//! own, which Save Groove writes and Load Groove reads.
//!
//! A groove file (`.blgroove`) is UTF-8 JSON, Beatlayer's own format:
//!
//! ```json
//! {"format":"beatlayer-groove","schema":1,"saved_by":"Beatlayer 0.2.0",
//!  "name":"Swing 16ths 60","length":2822400,
//!  "notes":[[0,127],[211680,100]],
//!  "base":"1/16","quantize":0.0,"timing":100.0,"random":0.0,"velocity":0.0}
//! ```
//!
//! `notes` lists each groove note as its position in ticks and its velocity.
//! A file that breaks the grooves spec's R1 or R2 doesn't load, and a file is
//! written whole or not at all, as a preset is ([`crate::preset`]).
//!
//! Grooves live in the User Library's `Grooves` folder, as Live's do.

use core::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bl_core::id::Id;
use bl_core::time::Ticks;
use bl_model::{Groove, GrooveBase, GrooveId, GrooveNote};
use serde::{Deserialize, Serialize};

use crate::preset::write_whole;

/// A groove file's extension.
pub const GROOVE_EXTENSION: &str = "blgroove";

/// The `format` field every groove file starts with.
pub const GROOVE_FORMAT: &str = "beatlayer-groove";

/// The groove schema this build writes, and the newest it reads.
pub const GROOVE_SCHEMA: u32 = 1;

/// The largest groove file read, in bytes: far more than a groove needs, so
/// a stray large file is refused rather than read.
pub const MAX_GROOVE_BYTES: u64 = 4 << 20;

/// A groove, as a groove file holds it: everything but its ID, which the
/// set gives it when it joins the pool.
#[derive(Clone, Debug, PartialEq)]
pub struct GrooveFile {
    /// The groove's name.
    pub name: String,
    /// Its length in ticks, positive.
    pub length: Ticks,
    /// Its notes, by position.
    pub notes: Vec<GrooveNote>,
    /// Its Base.
    pub base: GrooveBase,
    /// Quantize, from 0 to 100 percent.
    pub quantize: f64,
    /// Timing, from 0 to 100 percent.
    pub timing: f64,
    /// Random, from 0 to 100 percent.
    pub random: f64,
    /// Velocity, from -100 to +100 percent.
    pub velocity: f64,
}

#[derive(Serialize, Deserialize)]
struct File {
    format: String,
    schema: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    saved_by: String,
    name: String,
    length: Ticks,
    notes: Vec<(Ticks, u8)>,
    base: GrooveBase,
    quantize: f64,
    timing: f64,
    random: f64,
    velocity: f64,
}

/// Why a groove didn't load or save.
#[derive(Debug)]
#[non_exhaustive]
pub enum GrooveFileError {
    /// Reading or writing the file failed.
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        error: io::Error,
    },
    /// The file isn't a Beatlayer groove, is damaged, or breaks the grooves
    /// spec's limits.
    Invalid(String),
    /// The file was written by a newer Beatlayer.
    TooNew {
        /// Its schema version.
        found: u32,
    },
}

impl fmt::Display for GrooveFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Invalid(why) => write!(f, "not a Beatlayer groove: {why}"),
            Self::TooNew { found } => write!(
                f,
                "the groove was saved by a newer Beatlayer (groove schema {found}; this one \
                 reads up to {GROOVE_SCHEMA})"
            ),
        }
    }
}

impl std::error::Error for GrooveFileError {}

impl GrooveFile {
    /// `groove`'s contents.
    #[must_use]
    pub fn of(groove: &Groove) -> Self {
        Self {
            name: groove.name.clone(),
            length: groove.length,
            notes: groove.notes.clone(),
            base: groove.base,
            quantize: groove.quantize,
            timing: groove.timing,
            random: groove.random,
            velocity: groove.velocity,
        }
    }

    /// The groove with ID `id`.
    #[must_use]
    pub fn into_groove(self, id: GrooveId) -> Groove {
        Groove {
            id,
            name: self.name,
            length: self.length,
            notes: self.notes,
            base: self.base,
            quantize: self.quantize,
            timing: self.timing,
            random: self.random,
            velocity: self.velocity,
        }
    }

    /// The file's bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.encode_by(crate::SAVED_BY)
    }

    /// The file's bytes, naming `saved_by` as what wrote them; left empty,
    /// the file doesn't name one, so it stays the same from one version to
    /// the next (the bundled grooves' files).
    #[must_use]
    pub fn encode_by(&self, saved_by: &str) -> Vec<u8> {
        let file = File {
            format: GROOVE_FORMAT.to_owned(),
            schema: GROOVE_SCHEMA,
            saved_by: saved_by.to_owned(),
            name: self.name.clone(),
            length: self.length,
            notes: self
                .notes
                .iter()
                .map(|n| (n.position, n.velocity))
                .collect(),
            base: self.base,
            quantize: self.quantize,
            timing: self.timing,
            random: self.random,
            velocity: self.velocity,
        };
        let mut bytes = serde_json::to_vec_pretty(&file).unwrap_or_default();
        bytes.push(b'\n');
        bytes
    }

    /// A groove from a file's bytes.
    ///
    /// # Errors
    ///
    /// [`GrooveFileError::Invalid`] if they aren't a groove, or the groove
    /// breaks the grooves spec's R1 or R2, and [`GrooveFileError::TooNew`]
    /// if a newer Beatlayer wrote them.
    pub fn decode(bytes: &[u8]) -> Result<Self, GrooveFileError> {
        let invalid = |why: String| GrooveFileError::Invalid(why);
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        if value.get("format").and_then(serde_json::Value::as_str) != Some(GROOVE_FORMAT) {
            return Err(invalid(format!("its format isn't \"{GROOVE_FORMAT}\"")));
        }
        let schema = value
            .get("schema")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| invalid("it has no schema version".to_owned()))?;
        if schema > u64::from(GROOVE_SCHEMA) {
            return Err(GrooveFileError::TooNew {
                found: u32::try_from(schema).unwrap_or(u32::MAX),
            });
        }
        let file: File = serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
        let loaded = Self {
            name: file.name,
            length: file.length,
            notes: file
                .notes
                .into_iter()
                .map(|(position, velocity)| GrooveNote { position, velocity })
                .collect(),
            base: file.base,
            quantize: file.quantize,
            timing: file.timing,
            random: file.random,
            velocity: file.velocity,
        };
        // Spec: grooves R4. A file that breaks R1 or R2 doesn't load.
        let id = GrooveId(Id::new(1).ok_or_else(|| invalid("no ID".to_owned()))?);
        loaded
            .clone()
            .into_groove(id)
            .check()
            .map_err(|why| invalid(why.to_string()))?;
        Ok(loaded)
    }

    /// Reads a groove file.
    ///
    /// # Errors
    ///
    /// [`GrooveFileError::Io`] if it can't be read, and as
    /// [`GrooveFile::decode`].
    pub fn load(path: &Path) -> Result<Self, GrooveFileError> {
        let io = |error| GrooveFileError::Io {
            path: path.to_owned(),
            error,
        };
        let size = fs::metadata(path).map_err(io)?.len();
        if size > MAX_GROOVE_BYTES {
            return Err(GrooveFileError::Invalid(format!(
                "the file is {size} bytes, more than a groove holds"
            )));
        }
        Self::decode(&fs::read(path).map_err(io)?)
    }

    /// Writes the groove to `path`, making its folder if need be, whole or
    /// not at all.
    ///
    /// # Errors
    ///
    /// [`GrooveFileError::Io`] if it can't be written.
    pub fn save(&self, path: &Path) -> Result<(), GrooveFileError> {
        write_whole(path, &self.encode()).map_err(|error| match error {
            crate::preset::PresetError::Io { path, error } => GrooveFileError::Io { path, error },
            other => GrooveFileError::Invalid(other.to_string()),
        })
    }
}

/// The percentages the bundled swing grooves come in.
const SWINGS: [i64; 5] = [55, 60, 65, 70, 75];

/// A groove one 4/4 bar long over steps of `base`, the second step of each
/// pair of steps at `swing` percent of the pair's length (50 is straight). A
/// step on a beat has velocity 127 and the others 100.
fn stepped(name: String, base: GrooveBase, swing: i64) -> GrooveFile {
    let quarter = GrooveBase::Quarter.ticks().get();
    let step = base.ticks().get();
    let length = 4 * quarter;
    let notes = (0..length / step)
        .map(|n| {
            let line = n * step;
            GrooveNote {
                position: Ticks(if n % 2 == 1 {
                    // The pair began one step ago, and is two steps long.
                    line - step + 2 * step * swing / 100
                } else {
                    line
                }),
                velocity: if line % quarter == 0 { 127 } else { 100 },
            }
        })
        .collect();
    GrooveFile {
        name,
        length: Ticks(length),
        notes,
        base,
        quantize: 0.0,
        timing: 100.0,
        random: 0.0,
        velocity: 0.0,
    }
}

/// Beatlayer's bundled grooves, in the order the browser lists them (the
/// grooves spec R36): Swing 8ths and Swing 16ths at 55, 60, 65, 70 and 75
/// percent, and Straight 8ths and Straight 16ths, each one 4/4 bar long.
#[must_use]
pub fn bundled() -> Vec<GrooveFile> {
    let mut grooves = Vec::new();
    for (steps, base) in [
        ("8ths", GrooveBase::Eighth),
        ("16ths", GrooveBase::Sixteenth),
    ] {
        for swing in SWINGS {
            grooves.push(stepped(format!("Swing {steps} {swing}"), base, swing));
        }
        grooves.push(stepped(format!("Straight {steps}"), base, 50));
    }
    grooves
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: grooves R36
    #[test]
    fn a_swing_puts_the_second_step_of_each_pair_at_its_share_of_the_pair() {
        let bundled = bundled();
        assert_eq!(bundled.len(), 12);
        let find = |name: &str| bundled.iter().find(|g| g.name == name).expect(name);
        let swing = find("Swing 16ths 60");
        let at = |n: usize| (swing.notes[n].position.get(), swing.notes[n].velocity);
        assert_eq!(swing.notes.len(), 16);
        assert_eq!(swing.base, GrooveBase::Sixteenth);
        assert_eq!((swing.quantize, swing.timing), (0.0, 100.0));
        assert_eq!((swing.random, swing.velocity), (0.0, 0.0));
        assert_eq!(at(0), (0, 127));
        // 60 % of an eighth (352 800 ticks) is 211 680.
        assert_eq!(at(1), (211_680, 100));
        assert_eq!(at(2), (352_800, 100));
        assert_eq!(at(4), (705_600, 127));
        let straight = find("Straight 8ths");
        let positions: Vec<i64> = straight.notes.iter().map(|n| n.position.get()).collect();
        assert_eq!(positions[..3], [0, 352_800, 705_600]);
        assert_eq!(straight.notes.len(), 8);
        for groove in &bundled {
            assert_eq!(groove.length.get(), 2_822_400, "{}", groove.name);
            assert_eq!(&GrooveFile::decode(&groove.encode()).unwrap(), groove);
        }
    }

    fn groove() -> GrooveFile {
        GrooveFile {
            name: "Swing".to_owned(),
            length: Ticks::WHOLE,
            notes: vec![
                GrooveNote {
                    position: Ticks::ZERO,
                    velocity: 127,
                },
                GrooveNote {
                    position: Ticks::QUARTER,
                    velocity: 90,
                },
            ],
            base: GrooveBase::Eighth,
            quantize: 10.0,
            timing: 62.5,
            random: 0.0,
            velocity: -40.0,
        }
    }

    // Spec: grooves R4
    #[test]
    fn a_groove_file_round_trips() {
        let groove = groove();
        assert_eq!(GrooveFile::decode(&groove.encode()).unwrap(), groove);
        let folder = std::env::temp_dir().join(format!("bl-groove-{}", std::process::id()));
        let path = folder.join("Swing.blgroove");
        groove.save(&path).unwrap();
        assert_eq!(GrooveFile::load(&path).unwrap(), groove);
        // Whole or not at all: nothing is left beside it.
        let names: Vec<_> = fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["Swing.blgroove"]);
        fs::remove_file(&path).ok();
        fs::remove_dir(&folder).ok();
    }

    // Spec: grooves R4
    #[test]
    fn a_broken_groove_file_is_refused() {
        let good = String::from_utf8(groove().encode()).unwrap();
        let broken = |from: &str, to: &str| {
            let text = good.replace(from, to);
            assert_ne!(text, good, "{from}");
            GrooveFile::decode(text.as_bytes())
        };
        assert!(matches!(
            GrooveFile::decode(b"not json"),
            Err(GrooveFileError::Invalid(_))
        ));
        assert!(GrooveFile::decode(b"{}").is_err());
        assert!(broken("beatlayer-groove", "beatlayer-preset").is_err());
        assert!(matches!(
            broken("\"schema\": 1", "\"schema\": 2"),
            Err(GrooveFileError::TooNew { found: 2 })
        ));
        assert!(broken("\"base\": \"1/8\"", "\"base\": \"1/3\"").is_err());
        assert!(broken("\"quantize\": 10.0", "\"quantize\": 101.0").is_err());
        assert!(broken("\"velocity\": -40.0", "\"velocity\": -140.0").is_err());
        assert!(broken("\"length\": 2822400", "\"length\": 0").is_err());
        // A note past the groove's length, and a note with no velocity.
        assert!(broken("705600,", "2822400,").is_err());
        assert!(broken("127", "0").is_err());
        // Two notes at one position.
        assert!(broken("705600,", "0,").is_err());
        // A file cut short.
        assert!(GrooveFile::decode(&good.as_bytes()[..good.len() / 2]).is_err());
    }
}
