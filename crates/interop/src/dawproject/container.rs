//! Reading a DAWproject container (the DAWproject spec, R1, R2, R3 and R5):
//! the ZIP file's `project.xml` and `metadata.xml`, and the files it holds.
//!
//! The container is read safely: an entry whose name could escape the
//! folder it's written to is never read, an entry compressed by anything but
//! storing or deflate is left out, and no entry is read past the size its
//! header gives.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Cursor, Read};

use zip::CompressionMethod;

use super::model::Project;
use super::read::{self, ReadReport};

/// The most entries one container may hold (R5).
pub const MAX_ENTRIES: usize = 100_000;

/// The most bytes an XML part may unpack to (R5).
pub const MAX_XML_BYTES: u64 = 256 << 20;

/// The name the project part has in the container's root (R1).
pub const PROJECT_PART: &str = "project.xml";

/// The name the metadata part has in the container's root (R1).
pub const METADATA_PART: &str = "metadata.xml";

/// Why a DAWproject file was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The file isn't a ZIP file, so it isn't a DAWproject file (R1).
    NotAProject,
    /// The container has no `project.xml` in its root (R1).
    NoProject,
    /// The container holds more entries than the limit (R5).
    TooManyEntries(usize),
    /// An XML part is larger than the limit (R5).
    TooLarge {
        /// The part's name.
        part: String,
        /// Its size, unpacked.
        size: u64,
    },
    /// An XML part isn't in an encoding Beatlayer reads (R2).
    Encoding {
        /// The part's name.
        part: String,
    },
    /// The file's version isn't one Beatlayer reads (R3).
    Version {
        /// The version the file says.
        version: String,
    },
    /// The set couldn't be built.
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAProject => f.write_str("this isn't a DAWproject file: it isn't a ZIP file"),
            Self::NoProject => write!(
                f,
                "this isn't a DAWproject file: it has no {PROJECT_PART} in its root"
            ),
            Self::TooManyEntries(count) => write!(
                f,
                "this DAWproject file holds {count} entries, more than the {MAX_ENTRIES} Beatlayer \
                 reads"
            ),
            Self::TooLarge { part, size } => write!(
                f,
                "this DAWproject file's {part} unpacks to {size} bytes, more than the \
                 {MAX_XML_BYTES} Beatlayer reads"
            ),
            Self::Encoding { part } => write!(
                f,
                "this DAWproject file's {part} isn't in UTF-8 or UTF-16, which Beatlayer reads"
            ),
            Self::Version { version } => write!(
                f,
                "this DAWproject file is version {version}, which Beatlayer doesn't read; it reads \
                 version 1"
            ),
            Self::Failed(reason) => write!(f, "this DAWproject file couldn't be read: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

/// A DAWproject file, read.
#[derive(Clone, Debug, Default)]
pub struct Container {
    /// The project the file holds.
    pub project: Project,
    /// The container's files, by their path in it, with `/` between the
    /// parts.
    pub files: BTreeMap<String, Vec<u8>>,
    /// The metadata the file holds, as its text (R64).
    pub metadata: Option<String>,
    /// What the reader found it couldn't use.
    pub report: ReadReport,
}

/// Reads the DAWproject file in `bytes`.
///
/// # Errors
///
/// [`Error`] if the bytes aren't a DAWproject file Beatlayer reads.
pub fn read_container(bytes: &[u8]) -> Result<Container, Error> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| Error::NotAProject)?;
    if archive.len() > MAX_ENTRIES {
        return Err(Error::TooManyEntries(archive.len()));
    }
    let mut report = ReadReport::default();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut project_bytes: Option<Vec<u8>> = None;
    let mut metadata_bytes: Option<Vec<u8>> = None;

    for index in 0..archive.len() {
        let name = archive
            .name_for_index(index)
            .and_then(|name| name.ok())
            .map(|name| name.into_owned())
            .unwrap_or_default();
        let path = normalise(&name);
        if path.ends_with('/') {
            // A folder entry holds nothing.
            continue;
        }
        if !safe(&path) {
            // An entry that could escape the folder it's written to is never
            // read (R5).
            report.left_out.push(format!("the entry `{name}`"));
            continue;
        }
        if path == PROJECT_PART || path == METADATA_PART {
            let Some(file) = archive.by_index(index).ok() else {
                continue;
            };
            if file.size() > MAX_XML_BYTES {
                return Err(Error::TooLarge {
                    part: path.clone(),
                    size: file.size(),
                });
            }
            let size = file.size();
            let mut data = Vec::new();
            // No entry is read past the size its header gives (R5).
            if file.take(size).read_to_end(&mut data).is_err() {
                continue;
            }
            if path == PROJECT_PART {
                project_bytes = Some(data);
            } else {
                metadata_bytes = Some(data);
            }
            continue;
        }
        // Anything else is a file the project may use.
        let Some(file) = archive.by_index(index).ok() else {
            report.left_out.push(format!("the entry `{name}`"));
            continue;
        };
        if !matches!(
            file.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            // An entry compressed by anything else is left out (R5).
            report.left_out.push(format!("the entry `{name}`"));
            continue;
        }
        let size = file.size();
        let mut data = Vec::new();
        if file.take(size).read_to_end(&mut data).is_err() {
            report.left_out.push(format!("the entry `{name}`"));
            continue;
        }
        files.insert(path, data);
    }

    let Some(project_bytes) = project_bytes else {
        return Err(Error::NoProject);
    };
    let text = decode(&project_bytes, PROJECT_PART)?;
    let Some(root) = read::read_xml(&text, &mut report) else {
        return Err(Error::NoProject);
    };
    let Some(project) = read::project(&root, &mut report) else {
        return Err(Error::NoProject);
    };
    let metadata = match metadata_bytes {
        Some(bytes) => decode(&bytes, METADATA_PART).ok(),
        None => None,
    };
    Ok(Container {
        project,
        files,
        metadata,
        report,
    })
}

/// A container path with `/` between the parts, whatever the file used.
fn normalise(name: &str) -> String {
    name.replace('\\', "/")
}

/// Whether a container path is safe to read and write: not absolute, no
/// drive letter, and no `..` part (R5).
fn safe(path: &str) -> bool {
    if path.starts_with('/') || path.starts_with('\\') {
        return false;
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return false;
    }
    !path
        .split('/')
        .any(|part| part == ".." || part.is_empty() && path.len() > 1)
}

/// The text of an XML part: UTF-8 with or without a byte-order mark, or
/// UTF-16 with one (R2).
fn decode(bytes: &[u8], part: &str) -> Result<String, Error> {
    let encoding = |_| Error::Encoding {
        part: part.to_owned(),
    };
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(rest.to_vec()).map_err(|_| encoding(()));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, u16::from_le_bytes, part);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, u16::from_be_bytes, part);
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| encoding(()))
}

/// The text of a UTF-16 part.
fn utf16(bytes: &[u8], order: fn([u8; 2]) -> u16, part: &str) -> Result<String, Error> {
    if !bytes.len().is_multiple_of(2) {
        return Err(Error::Encoding {
            part: part.to_owned(),
        });
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| order([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| Error::Encoding {
        part: part.to_owned(),
    })
}
