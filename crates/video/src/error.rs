//! The video error type.

use std::fmt;
use std::io;

use bl_media::MediaError;

/// Why reading or decoding a movie failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum VideoError {
    /// Reading the file failed.
    Io(io::Error),
    /// The file isn't a QuickTime movie, or is one Beatlayer doesn't read
    /// (for example a fragmented movie).
    Unsupported(String),
    /// The file claims to be a movie but its boxes are damaged.
    Malformed(String),
    /// The movie has no video track.
    NoVideo,
    /// The movie has no audio track.
    NoAudio,
    /// No decoder on this computer takes the movie's video codec (R1).
    NoDecoder {
        /// The codec's four-character code, as the movie names it.
        codec: String,
        /// Why every decoder refused it, for the person to read.
        reason: String,
    },
    /// A decoder failed on a frame.
    Decode(String),
    /// The movie's audio failed to decode.
    Audio(MediaError),
}

impl fmt::Display for VideoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "couldn't read the movie: {e}"),
            Self::Unsupported(what) => write!(f, "unsupported movie: {what}"),
            Self::Malformed(what) => write!(f, "damaged or malformed movie: {what}"),
            Self::NoVideo => f.write_str("the movie has no video track"),
            Self::NoAudio => f.write_str("the movie has no audio track"),
            Self::NoDecoder { codec, reason } => {
                write!(f, "no decoder takes the video codec {codec}: {reason}")
            }
            Self::Decode(what) => write!(f, "couldn't decode a frame: {what}"),
            Self::Audio(e) => write!(f, "the movie's sound: {e}"),
        }
    }
}

impl std::error::Error for VideoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Audio(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for VideoError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<MediaError> for VideoError {
    fn from(e: MediaError) -> Self {
        Self::Audio(e)
    }
}
