//! Extension packages (extensions R1 to R3): a `.blext` ZIP file holding
//! `extension.toml` and the components it names, read and checked whole
//! before anything is kept.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Read, Seek};
use std::path::Path;

use serde::Deserialize;
use wasmtime::Engine;
use wasmtime::component::{Component, types::ComponentItem};
use zip::ZipArchive;

use crate::Error;

/// The manifest's name inside a package.
pub const MANIFEST: &str = "extension.toml";

/// The world version this Beatlayer implements (R2): the WIT package
/// `beatlayer:ext` under `wit/`.
pub const WORLD: Version = Version(0, 1, 0);

/// The most unpacked bytes a package may hold.
const MAX_BYTES: u64 = 128 << 20;
/// The most files a package may hold.
const MAX_FILES: usize = 4096;
/// The most memory an instance may reserve (R9), in MiB.
pub const MAX_MEMORY_MIB: u32 = 64;
/// What the manifest asks for when it doesn't say.
const DEFAULT_MEMORY_MIB: u32 = 16;
/// The imports a manifest may ask for (R1, R38).
const IMPORTS: &[&str] = &["set"];
/// The imports this Beatlayer provides: none until the API to the set (R38).
const PROVIDED: &[&str] = &[];

/// A version, `major.minor.patch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// Parses `1.2.3`; nothing else is a version.
    ///
    /// # Errors
    ///
    /// Why `text` isn't one.
    pub fn parse(text: &str) -> Result<Self, String> {
        let number = |part: &str| {
            let plain = !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part == "0" || !part.starts_with('0'));
            plain.then(|| part.parse().ok()).flatten()
        };
        let parts: Vec<Option<u32>> = text.split('.').map(number).collect();
        if let [Some(major), Some(minor), Some(patch)] = parts[..] {
            Ok(Self(major, minor, patch))
        } else {
            Err(format!("`{text}` isn't a version like 1.2.3"))
        }
    }

    /// Whether a host at this version accepts a component built for `other`:
    /// the same major version, and no later than this (R2).
    #[must_use]
    pub fn accepts(self, other: Self) -> bool {
        self.0 == other.0 && other <= self
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// What a component does (R1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A MIDI tool.
    MidiTool,
    /// A control surface.
    ControlSurface,
    /// An audio effect device.
    AudioEffect,
    /// An instrument device.
    Instrument,
    /// A MIDI effect device.
    MidiEffect,
}

impl Kind {
    /// Every kind.
    pub const ALL: [Self; 5] = [
        Self::MidiTool,
        Self::ControlSurface,
        Self::AudioEffect,
        Self::Instrument,
        Self::MidiEffect,
    ];

    /// The kind's name in the manifest and in its world.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MidiTool => "midi-tool",
            Self::ControlSurface => "control-surface",
            Self::AudioEffect => "audio-effect",
            Self::Instrument => "instrument",
            Self::MidiEffect => "midi-effect",
        }
    }
}

/// One component of an extension, as its manifest describes it (R1).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ComponentSpec {
    /// The component's file, relative to the package's root.
    pub file: String,
    /// What it does.
    pub kind: Kind,
    /// The version of the `beatlayer:ext` world it targets.
    pub world: Version,
    /// The imports it asks for (R38).
    #[serde(default)]
    pub imports: Vec<String>,
    /// The memory its instances reserve up front, in MiB (R9).
    #[serde(default = "default_memory")]
    pub memory_mib: u32,
}

const fn default_memory() -> u32 {
    DEFAULT_MEMORY_MIB
}

impl ComponentSpec {
    /// The name of the interface a component of this kind and world
    /// exports, which holds its functions: `beatlayer:ext/midi-tool@0.1.0`.
    #[must_use]
    pub fn interface(&self) -> String {
        format!("beatlayer:ext/{}@{}", self.kind.name(), self.world)
    }
}

/// An extension's manifest, `extension.toml` (R1).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Its ID, reverse-domain, such as `org.example.tools`.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: Version,
    /// Who made it.
    pub author: String,
    /// The licence it's under.
    pub licence: String,
    /// What it does.
    pub description: String,
    /// Its components.
    pub components: Vec<ComponentSpec>,
}

impl Manifest {
    /// Parses and checks a manifest.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] with the first thing wrong with it.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let manifest: Self = toml::from_str(text)
            .map_err(|e| refuse(format!("{MANIFEST} doesn't parse: {}", e.message())))?;
        manifest.check()?;
        Ok(manifest)
    }

    fn check(&self) -> Result<(), Error> {
        check_id(&self.id)?;
        for (field, value) in [
            ("name", &self.name),
            ("author", &self.author),
            ("licence", &self.licence),
            ("description", &self.description),
        ] {
            if value.trim().is_empty() {
                return Err(refuse(format!("{MANIFEST} gives an empty {field}")));
            }
        }
        if self.components.is_empty() {
            return Err(refuse(format!("{MANIFEST} names no components")));
        }
        let mut files = BTreeSet::new();
        for component in &self.components {
            let file = &component.file;
            check_path(file).map_err(|why| refuse(format!("the component `{file}` {why}")))?;
            if !files.insert(file.to_lowercase()) {
                return Err(refuse(format!("{MANIFEST} names `{file}` twice")));
            }
            if !WORLD.accepts(component.world) {
                return Err(refuse(format!(
                    "`{file}` targets world version {}, and this Beatlayer accepts {WORLD} and \
                     earlier of major version {}",
                    component.world, WORLD.0
                )));
            }
            if let Some(import) = component
                .imports
                .iter()
                .find(|i| !IMPORTS.contains(&i.as_str()))
            {
                return Err(refuse(format!(
                    "`{file}` asks for the import `{import}`, which no world has"
                )));
            }
            if !(1..=MAX_MEMORY_MIB).contains(&component.memory_mib) {
                return Err(refuse(format!(
                    "`{file}` asks for {} MiB of memory; a component may have 1 to \
                     {MAX_MEMORY_MIB}",
                    component.memory_mib
                )));
            }
        }
        Ok(())
    }
}

/// A package that passed every check, ready to install (R3). Reading it
/// wrote nothing.
pub struct Package {
    /// Its manifest.
    pub manifest: Manifest,
    /// Every file it holds, by path.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Each component's compiled code, in the manifest's order.
    pub compiled: Vec<Vec<u8>>,
}

impl Package {
    /// Reads and checks a package: its paths, its manifest, and each
    /// component (R3). `engine` compiles the components, which is how they
    /// are checked, and gives the code to keep (R5).
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] with the reason the whole package is refused, or
    /// [`Error::Io`] when the file can't be read.
    pub fn read<R: Read + Seek>(reader: R, engine: &Engine) -> Result<Self, Error> {
        let files = unzip(reader)?;
        let manifest = files
            .get(MANIFEST)
            .ok_or_else(|| refuse(format!("the package holds no {MANIFEST} at its root")))?;
        let manifest =
            std::str::from_utf8(manifest).map_err(|_| refuse(format!("{MANIFEST} isn't text")))?;
        let manifest = Manifest::parse(manifest)?;
        let mut compiled = Vec::new();
        for spec in &manifest.components {
            let bytes = files.get(&spec.file).ok_or_else(|| {
                refuse(format!(
                    "{MANIFEST} names `{}`, which the package lacks",
                    spec.file
                ))
            })?;
            compiled.push(check_component(engine, spec, bytes)?);
        }
        Ok(Self {
            manifest,
            files,
            compiled,
        })
    }
}

/// Compiles the component in `bytes`, checks it against `spec`, and gives
/// its compiled code.
fn check_component(engine: &Engine, spec: &ComponentSpec, bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let file = &spec.file;
    let component = Component::new(engine, bytes)
        .map_err(|e| refuse(format!("`{file}` isn't a valid component: {e:#}")))?;
    let ty = component.component_type();
    let interface = spec.interface();
    let exported = ty.exports(engine).any(|(name, item)| {
        name == interface && matches!(item.ty, ComponentItem::ComponentInstance(_))
    });
    if !exported {
        return Err(refuse(format!(
            "`{file}` doesn't export `{interface}`, the {} world at version {}",
            spec.kind.name(),
            spec.world
        )));
    }
    // R8: a component reaches only what its world gives it.
    for (name, _) in ty.imports(engine) {
        let provided = name
            .strip_prefix("beatlayer:ext/")
            .and_then(|rest| rest.split('@').next())
            .filter(|import| {
                spec.imports.iter().any(|asked| asked == import) && PROVIDED.contains(import)
            });
        if provided.is_none() {
            return Err(refuse(format!(
                "`{file}` imports `{name}`, which this Beatlayer doesn't give a {} component",
                spec.kind.name()
            )));
        }
    }
    component
        .serialize()
        .map_err(|e| refuse(format!("`{file}` couldn't be compiled: {e:#}")))
}

fn refuse(reason: String) -> Error {
    Error::Refused(reason)
}

/// Reads every file of the archive, refusing an unsafe path (R3), a link,
/// two names that differ only in case, and more than the limits allow.
fn unzip<R: Read + Seek>(reader: R) -> Result<BTreeMap<String, Vec<u8>>, Error> {
    let mut archive = ZipArchive::new(reader)
        .map_err(|e| refuse(format!("the package isn't a ZIP file: {e}")))?;
    if archive.len() > MAX_FILES {
        return Err(refuse(format!(
            "the package holds more than {MAX_FILES} files"
        )));
    }
    let (mut files, mut seen, mut total) = (BTreeMap::new(), BTreeSet::new(), 0u64);
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| refuse(format!("the package's entry {index} can't be read: {e}")))?;
        let name = entry
            .name()
            .map_err(|e| {
                refuse(format!(
                    "the package's entry {index} has no usable name: {e}"
                ))
            })?
            .trim_end_matches('/')
            .to_owned();
        check_path(&name).map_err(|why| refuse(format!("the package's `{name}` {why}")))?;
        if entry.is_symlink() {
            return Err(refuse(format!("the package's `{name}` is a link")));
        }
        if !seen.insert(name.to_lowercase()) {
            return Err(refuse(format!("the package holds `{name}` twice")));
        }
        if entry.is_dir() {
            continue;
        }
        let size = entry.size();
        total += size;
        if total > MAX_BYTES {
            return Err(refuse(format!(
                "the package unpacks to more than {} MiB",
                MAX_BYTES >> 20
            )));
        }
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(size + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| refuse(format!("the package's `{name}` is damaged: {e}")))?;
        if bytes.len() as u64 != size {
            return Err(refuse(format!(
                "the package's `{name}` unpacks to a different size than it says"
            )));
        }
        files.insert(name, bytes);
    }
    Ok(files)
}

/// Why `path` can't be a file inside a package, if it can't: it must be
/// relative, climb nowhere, and be a name every platform allows.
fn check_path(path: &str) -> Result<(), &'static str> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if path.is_empty() {
        return Err("has no name");
    }
    if path.starts_with('/') || path.starts_with('\\') || Path::new(path).has_root() {
        return Err("has an absolute path");
    }
    for part in path.split('/') {
        let stem = part.split('.').next().unwrap_or_default();
        if part == ".." {
            return Err("climbs out of the package");
        }
        if part.is_empty() || part == "." {
            return Err("has an empty part in its path");
        }
        if part.len() > 255
            || part.ends_with(['.', ' '])
            || part.contains(['<', '>', ':', '"', '\\', '|', '?', '*'])
            || part.chars().any(char::is_control)
            || RESERVED.iter().any(|name| stem.eq_ignore_ascii_case(name))
        {
            return Err("has a part that can't be a file name");
        }
    }
    Ok(())
}

/// An ID is reverse-domain: two or more dot-separated parts of lowercase
/// letters, digits, `-` and `_`. It names the extension's folder, so nothing
/// else is allowed.
fn check_id(id: &str) -> Result<(), Error> {
    let part = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    };
    if id.len() > 128 || !id.contains('.') || !id.split('.').all(part) || check_path(id).is_err() {
        return Err(refuse(format!(
            "`{id}` isn't an extension ID: use a reverse domain like org.example.tools, in \
             lowercase letters, digits, - and _"
        )));
    }
    Ok(())
}
