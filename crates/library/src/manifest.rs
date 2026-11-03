//! The manifest: TOML files under `library/` (spec: core-library R1, R2).

use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The licences an item may have (R2): Beatlayer's own items are CC0.
pub const ALLOWED_LICENCES: [&str; 2] = ["CC0-1.0", "CC-BY-4.0"];

/// What an item is (R1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A multisampled instrument.
    Instrument,
    /// A device preset.
    Preset,
    /// A rack.
    Rack,
    /// A drum kit.
    DrumKit,
    /// A loop.
    Loop,
    /// A MIDI clip.
    MidiClip,
    /// A groove.
    Groove,
    /// A chord bank.
    ChordBank,
    /// A one-shot sample.
    OneShot,
    /// A texture.
    Texture,
}

/// Where an item comes from (R1): a URL with its checksum, or a script that
/// generates it from a seed; a script may also convert what a URL gives.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Source {
    /// Where the source recording is.
    pub url: Option<String>,
    /// The SHA-256 of what the URL gives, or of the script when there is no
    /// URL, in hex.
    pub sha256: String,
    /// The script under `library/scripts/` that generates or converts it.
    pub script: Option<String>,
    /// The script's seed.
    pub seed: u64,
    /// Whether it's a recording Beatlayer made, hosted at the URL.
    pub recorded: bool,
}

/// What a licence asks to be credited (R11).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Attribution {
    /// Who made it.
    pub creator: String,
    /// Its title.
    pub title: String,
    /// Where it's from.
    pub link: String,
    /// What was changed from the original.
    pub changes: String,
}

/// An item as the manifest lists it (R1).
#[derive(Clone, Debug, Deserialize)]
pub struct ItemDef {
    /// Its ID.
    pub id: String,
    /// Its name.
    pub name: String,
    /// What it is.
    pub kind: Kind,
    /// The package it goes in.
    pub package: String,
    /// The file it's built as, in the package.
    pub file: String,
    /// Its licence, as an SPDX identifier.
    #[serde(default)]
    pub licence: String,
    /// Its tags, each `Group|Tag` or `Group|Tag|Subtag`.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Where it comes from.
    #[serde(default)]
    pub source: Source,
    /// Its credit.
    #[serde(default)]
    pub attribution: Attribution,
}

/// A package of the library (R5): a part, installed as a unit.
#[derive(Clone, Debug, Deserialize)]
pub struct PackageDef {
    /// Its ID in the manifest: `core`, `instruments`, `drums`, `loops`.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// What it holds, for the download page.
    pub purpose: String,
}

/// Every TOML file of the manifest, together.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Manifest {
    /// The packages.
    #[serde(default)]
    pub package: Vec<PackageDef>,
    /// The items.
    #[serde(default)]
    pub item: Vec<ItemDef>,
}

/// The SHA-256 of `bytes`, in hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Manifest {
    /// Reads the `.toml` files of `dir` (not its subfolders), in name order.
    ///
    /// # Errors
    ///
    /// A message if a file can't be read or isn't a manifest.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();
        let mut all = Self::default();
        for file in files {
            let text =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let part: Self =
                toml::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
            all.package.extend(part.package);
            all.item.extend(part.item);
        }
        Ok(all)
    }

    /// What's wrong with `package`'s items, each naming its item (R2); empty
    /// if the package can be built. `dir` is where `scripts/` is.
    #[must_use]
    pub fn problems(&self, package: &str, dir: &Path) -> Vec<String> {
        let mut problems = Vec::new();
        if !self.package.iter().any(|p| p.id == package) {
            problems.push(format!("no package {package} in the manifest"));
        }
        for (i, item) in self.item.iter().enumerate() {
            if item.package != package {
                continue;
            }
            if self.item[..i].iter().any(|other| other.id == item.id) {
                problems.push(format!("item {}: two items have this ID", item.id));
            }
            item.problems(dir, &mut problems);
        }
        problems
    }

    /// The problems of every package, and of items in none.
    #[must_use]
    pub fn all_problems(&self, dir: &Path) -> Vec<String> {
        let mut problems: Vec<String> = self
            .package
            .iter()
            .flat_map(|p| self.problems(&p.id, dir))
            .collect();
        problems.extend(
            self.item
                .iter()
                .filter(|i| !self.package.iter().any(|p| p.id == i.package))
                .map(|i| format!("item {}: no package {}", i.id, i.package)),
        );
        problems
    }
}

fn plain(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.')
}

impl ItemDef {
    fn problems(&self, dir: &Path, out: &mut Vec<String>) {
        let mut bad = |why: String| out.push(format!("item {}: {why}", self.id));
        if !plain(&self.id) {
            bad("its ID may hold letters, digits, - _ and . only".to_owned());
        }
        if !plain(&self.file) {
            bad(format!(
                "its file name {:?} may hold letters, digits, - _ and . only",
                self.file
            ));
        }
        if !ALLOWED_LICENCES.contains(&self.licence.as_str()) {
            bad(format!(
                "licence {:?} isn't allowed (only {})",
                self.licence,
                ALLOWED_LICENCES.join(" and ")
            ));
        }
        let a = &self.attribution;
        if self.licence == "CC-BY-4.0" {
            for (field, value) in [
                ("creator", &a.creator),
                ("title", &a.title),
                ("link", &a.link),
                ("changes", &a.changes),
            ] {
                if value.trim().is_empty() {
                    bad(format!("CC BY 4.0 needs an attribution {field}"));
                }
            }
        }
        let s = &self.source;
        if s.sha256.len() != 64 || !s.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            bad("it has no checksum (source.sha256 is 64 hex digits)".to_owned());
        }
        if s.url.is_none() && s.script.is_none() {
            bad("it names neither a source URL nor a generating script".to_owned());
        }
        if let Some(script) = &s.script
            && (!plain(script) || !dir.join("scripts").join(script).is_file())
        {
            bad(format!("no script {script} in scripts/"));
        }
        if self.tags.is_empty() {
            bad("it has no tags".to_owned());
        }
        for tag in &self.tags {
            let parts: Vec<_> = tag.split('|').collect();
            if !(2..=3).contains(&parts.len()) || parts.iter().any(|p| p.trim().is_empty()) {
                bad(format!("tag {tag:?} isn't Group|Tag or Group|Tag|Subtag"));
            }
        }
    }
}
