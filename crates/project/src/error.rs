//! Why a set couldn't be loaded or saved.

use core::fmt;
use std::io;
use std::path::PathBuf;

/// Why a set couldn't be loaded.
///
/// A failed load changes nothing on disk.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadError {
    /// The file couldn't be read.
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// A project folder was given, and it holds no set.
    NoSet {
        /// The folder.
        folder: PathBuf,
    },
    /// A project folder was given, and it holds more than one set, so the
    /// set to open has to be named.
    SeveralSets {
        /// The sets in the folder.
        sets: Vec<PathBuf>,
    },
    /// The data isn't a Beatlayer set at all: it isn't zstd-compressed, or
    /// the JSON inside doesn't say it's a set.
    NotASet,
    /// The data is damaged: the compressed stream or the JSON inside it is
    /// broken, truncated, or fails its checksum.
    Corrupt {
        /// What's wrong.
        reason: String,
    },
    /// The set's JSON is larger than the loader accepts.
    TooLarge {
        /// The limit, in bytes of JSON.
        limit: usize,
    },
    /// The set was saved by a newer version of Beatlayer.
    TooNew {
        /// The schema version in the file.
        found: u32,
        /// The newest schema version this build reads.
        newest: u32,
    },
    /// The set's schema version is older than any this build can migrate.
    TooOld {
        /// The schema version in the file.
        found: u32,
        /// The oldest schema version this build reads.
        oldest: u32,
    },
    /// A migration from an older schema version failed.
    Migration {
        /// The version the failing migration reads.
        from: u32,
        /// Why it failed.
        message: String,
    },
    /// The JSON is well formed but breaks a rule of the set's schema or the
    /// document model: a missing field, a value out of range, a track in the
    /// wrong place.
    Invalid {
        /// What's wrong.
        reason: String,
    },
}

impl LoadError {
    pub(crate) fn corrupt(reason: impl fmt::Display) -> Self {
        Self::Corrupt {
            reason: reason.to_string(),
        }
    }

    pub(crate) fn invalid(reason: impl fmt::Display) -> Self {
        Self::Invalid {
            reason: reason.to_string(),
        }
    }

    /// Sorts a JSON error into broken JSON, which is damage, and JSON that
    /// breaks a rule.
    pub(crate) fn from_json(error: &serde_json::Error) -> Self {
        match error.classify() {
            serde_json::error::Category::Data => Self::invalid(error),
            serde_json::error::Category::Io
            | serde_json::error::Category::Syntax
            | serde_json::error::Category::Eof => Self::corrupt(error),
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "couldn't read {}: {source}", path.display()),
            Self::NoSet { folder } => write!(f, "{} holds no set", folder.display()),
            Self::SeveralSets { sets } => {
                write!(
                    f,
                    "the folder holds {} sets; name the one to open",
                    sets.len()
                )
            }
            Self::NotASet => f.write_str("this isn't a Beatlayer set"),
            Self::Corrupt { reason } => write!(f, "the set is damaged: {reason}"),
            Self::TooLarge { limit } => {
                write!(f, "the set is larger than the {limit}-byte limit")
            }
            Self::TooNew { found, newest } => write!(
                f,
                "the set was saved by a newer version of Beatlayer (schema {found}; this version reads up to {newest})"
            ),
            Self::TooOld { found, oldest } => write!(
                f,
                "the set's schema version {found} is older than any this version reads (the oldest is {oldest})"
            ),
            Self::Migration { from, message } => write!(
                f,
                "couldn't update the set from schema {from} to {}: {message}",
                from + 1
            ),
            Self::Invalid { reason } => write!(f, "the set isn't valid: {reason}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Why a set couldn't be saved.
///
/// A failed save leaves the file it would have replaced as it was.
#[derive(Debug)]
#[non_exhaustive]
pub enum SaveError {
    /// A file or folder couldn't be written.
    Io {
        /// The file or folder.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// The set couldn't be turned into JSON.
    Encode {
        /// What went wrong.
        reason: String,
    },
    /// The path has no file name to save to.
    NoFileName {
        /// The path.
        path: PathBuf,
    },
    /// The set has been saved so many times that its revision number can't
    /// go up again.
    RevisionsExhausted,
}

impl SaveError {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "couldn't write {}: {source}", path.display()),
            Self::Encode { reason } => write!(f, "couldn't encode the set: {reason}"),
            Self::NoFileName { path } => write!(f, "{} names no file to save to", path.display()),
            Self::RevisionsExhausted => f.write_str("the set's revision number can't go up again"),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
