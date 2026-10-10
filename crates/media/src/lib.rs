//! Media for Beatlayer: decoding, sample-rate conversion, content hashing,
//! and the media store. Peak files (WP-1.2), disk streaming (WP-1.3), and
//! encoding (WP-1.12) build on this crate.
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
//!
//! Nothing here runs on an audio thread: decoding reads files and allocates.
//! The engine receives `Arc<SampleData>`, which it reads without locks.

mod decode;
mod error;
mod hash;
mod mp4;
mod resample;
mod rf64;
mod sample_data;
mod store;

pub use decode::{Codec, Container, Decoder, MediaInfo, decode_file, probe};
pub use error::MediaError;
pub use hash::{ContentHash, ParseContentHashError};
pub use resample::{RateConverter, convert_sample_rate, converted_len};
pub use sample_data::SampleData;
pub use store::{MediaEntry, MediaStatus, MediaStore};
