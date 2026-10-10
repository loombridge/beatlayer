//! The media error type.

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::hash::ContentHash;

/// Why a media operation failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum MediaError {
    /// Reading or seeking the file failed.
    Io(io::Error),
    /// The file is in a format or codec Beatlayer doesn't read.
    Unsupported(String),
    /// The file claims a format it doesn't follow, or its data is damaged.
    Malformed(String),
    /// The file has no audio track.
    NoAudio,
    /// The media store has no entry for this hash.
    UnknownMedia(ContentHash),
    /// The file the media store knows for this hash isn't there any more.
    Missing {
        /// The media that is missing.
        hash: ContentHash,
        /// Where the store last found it.
        path: PathBuf,
    },
    /// The file at the known path no longer has the content this hash names.
    Changed {
        /// The media the store expected.
        hash: ContentHash,
        /// The file that changed.
        path: PathBuf,
        /// The content the file has now.
        now: ContentHash,
    },
    /// A sample-rate conversion couldn't be set up or run.
    Resample(String),
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "couldn't read the file: {e}"),
            Self::Unsupported(what) => write!(f, "unsupported media: {what}"),
            Self::Malformed(what) => write!(f, "damaged or malformed media: {what}"),
            Self::NoAudio => f.write_str("the file has no audio track"),
            Self::UnknownMedia(hash) => write!(f, "the media store has no media {hash}"),
            Self::Missing { hash, path } => {
                write!(f, "media {hash} is missing from {}", path.display())
            }
            Self::Changed { hash, path, now } => write!(
                f,
                "{} no longer holds media {hash}; its content is now {now}",
                path.display()
            ),
            Self::Resample(what) => write!(f, "sample-rate conversion failed: {what}"),
        }
    }
}

impl std::error::Error for MediaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for MediaError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Converts a symphonia error without making symphonia part of the public API.
pub(crate) fn from_symphonia(e: symphonia::core::errors::Error) -> MediaError {
    use symphonia::core::errors::Error;
    match e {
        Error::IoError(e) => MediaError::Io(e),
        Error::Unsupported(what) => MediaError::Unsupported(what.to_owned()),
        Error::DecodeError(what) | Error::LimitError(what) => {
            MediaError::Malformed(what.to_owned())
        }
        Error::SeekError(kind) => MediaError::Malformed(format!("seek failed: {kind:?}")),
        Error::ResetRequired => MediaError::Unsupported("the stream changes format mid-way".into()),
        other => MediaError::Malformed(other.to_string()),
    }
}
