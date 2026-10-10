//! Media for Beatlayer: decoding, sample-rate conversion, content hashing,
//! the media store, and peak files. Disk streaming (WP-1.3) and encoding
//! (WP-1.12) build on this crate.
//!
//! - [`Decoder`] reads WAV (including Broadcast Wave), RF64 and BW64 over
//!   4 GB, AIFF and AIFF-C, FLAC, MP3, Ogg Vorbis, and MPEG-4 with AAC or
//!   ALAC, to 32-bit float, a block at a time, with sample-accurate seeking.
//!   [`decode_file`] reads a whole file into [`SampleData`]. Symphonia does
//!   the decoding, except for RF64 and BW64, which this crate parses itself.
//!   Encoder delay and padding are trimmed, so a decoded file has the length
//!   and timing of what was encoded.
//! - [`RateConverter`] and [`convert_sample_rate`] convert between sample
//!   rates with aliasing below -100 dB in the passband, through rubato.
//! - [`ContentHash`] names media by the BLAKE3 hash of the file's bytes.
//! - [`MediaStore`] maps hashes to files, checks files still hold what they
//!   should, relinks moved files by content, and hands out decoded audio as
//!   shared [`SampleData`].
//! - [`Peaks`] are multi-resolution min/max overviews for drawing waveforms,
//!   laid out for the GPU. [`PeakCache`] builds them in the background, keyed
//!   by content hash, saves them in the platform's cache folder, and drops
//!   them when their source file changes. See [`peaks`] for the layout.
//!
//! Nothing here runs on an audio thread: decoding reads files and allocates.
//! The engine receives `Arc<SampleData>`, which it reads without locks.

mod decode;
mod error;
mod hash;
mod mp4;
pub mod peaks;
mod resample;
mod rf64;
mod sample_data;
mod store;

pub use decode::{Codec, Container, Decoder, MediaInfo, decode_file, probe};
pub use error::MediaError;
pub use hash::{ContentHash, ParseContentHashError};
pub use peaks::{PeakCache, PeakCacheOptions, PeakEvent, PeakStatus, Peaks};
pub use resample::{RateConverter, convert_sample_rate, converted_len};
pub use sample_data::SampleData;
pub use store::{MediaEntry, MediaStatus, MediaStore};
