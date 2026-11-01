//! Video for Beatlayer (WP-9.24, [the video spec](../../../docs/specs/video.md)):
//! QuickTime movies read, frames decoded ahead of the playhead, and the
//! movie's sound through `bl-media`.
//!
//! - [`Movie`] reads a QuickTime movie's tracks, sample tables and edit lists
//!   with a demuxer of its own. Times are exact [`Rational`]s through the edit
//!   list, and the frame shown at a time is the last one at or before it
//!   (R1, R2).
//! - [`VideoDecoder`] decodes the video through one trait: Motion JPEG in
//!   Rust, so generated test movies decode on every computer; H.264, HEVC and
//!   ProRes through VideoToolbox on macOS and Media Foundation on Windows; on
//!   Linux, FFmpeg's shared libraries, loaded at run time where installed
//!   (R26). Nothing is downloaded.
//! - [`VideoThread`] decodes frames ahead of the time a window asks for into
//!   a cache, and [`FrameSource`] does the same on the calling thread, for
//!   export (R27). Neither is ever called from an audio thread.
//! - [`MovieAudio`] reads the movie's sound through `bl-media`, as an audio
//!   file is read, with silence in its gaps (R3, R4).
//!
//! A damaged movie is refused, or its damaged frames come out black, without a
//! crash (R28).

mod annexb;
mod audio;
mod boxes;
mod convert;
mod decoder;
mod edits;
mod error;
#[cfg(unix)]
pub mod ffmpeg;
mod markers;
#[cfg(windows)]
mod mediafoundation;
mod mjpeg;
mod movie;
mod player;
mod rational;
#[cfg(target_os = "macos")]
mod videotoolbox;

pub use audio::MovieAudio;
pub use decoder::{CodedSample, Frame, VideoDecoder, open_decoder};
pub use edits::{Edit, EditList};
pub use error::VideoError;
pub use markers::Marker;
pub use movie::{AudioTrack, MAX_DIMENSION, MAX_PIXELS, Movie, ReadSeek, Sample, VideoTrack};
pub use player::{FrameSource, Picture, VideoThread};
pub use rational::Rational;
