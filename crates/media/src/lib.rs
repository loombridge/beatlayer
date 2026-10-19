//! Media for Beatlayer: decoding, encoding, sample-rate conversion, content
//! hashing, the media store, peak files, and disk streaming.
//!
//! - [`Decoder`] reads WAV (including Broadcast Wave), RF64 and BW64 over
//!   4 GB, AIFF and AIFF-C, FLAC, MP3, Ogg Vorbis, and MPEG-4 with AAC or
//!   ALAC, to 32-bit float, a block at a time, with sample-accurate seeking.
//!   [`decode_file`] reads a whole file into [`SampleData`]. Symphonia does
//!   the decoding, except for RF64 and BW64, which this crate parses itself.
//!   Encoder delay and padding are trimmed, so a decoded file has the length
//!   and timing of what was encoded.
//! - [`AudioWriter`] writes WAV, AIFF, and FLAC at 16, 24, or 32 bits, a
//!   block at a time, with a [`Quantiser`] that dithers and rounds below 32
//!   bits, and MP3 at 320 kbps CBR (WP-1.12, WP-2.26, the export spec). See
//!   [`encode`].
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
//! - [`stream`]: disk streaming. A [`Streamer`]'s disk thread reads long
//!   files ahead of the playhead, and an [`AudioSource`] ([`StreamSource`]
//!   or [`MemorySource`]) is what a clip player reads on the audio thread.
//!
//! A header that declares more than [`limits`] allows is refused before
//! anything is allocated for it.
//!
//! Decoding reads files and allocates, so it never runs on an audio thread.
//! The engine receives `Arc<SampleData>`, which it reads without locks, or
//! an [`AudioSource`], whose reads are real-time safe.

mod decode;
pub mod edits;
pub mod encode;
mod error;
mod hash;
pub mod limits;
mod mp4;
pub mod peaks;
mod progressive;
mod resample;
mod rf64;
mod sample_data;
mod sampler_chunk;
mod store;
pub mod stream;

pub use decode::{Codec, Container, Decoder, MediaInfo, decode_file, probe};
pub use edits::{Edit, EditKind};
pub use encode::{
    AudioWriter, BitDepth, Dither, EncodeSpec, FileType, MP3_RATES, Quantiser, wav_bytes,
};
pub use error::MediaError;
pub use hash::{ContentHash, ParseContentHashError};
pub use peaks::{PeakCache, PeakCacheOptions, PeakEvent, PeakStatus, Peaks, write_atomically};
pub use progressive::{Fill, ProgressiveSample};
pub use resample::{RateConverter, convert_sample_rate, converted_len};
pub use sample_data::SampleData;
pub use sampler_chunk::{SamplerInfo, read_sampler_info};
pub use store::{MediaEntry, MediaStatus, MediaStore};
pub use stream::{
    AudioSource, Filled, MemorySource, PlanarMut, Source, StreamConfig, StreamId, StreamSource,
    StreamStats, Streamer, StreamerStats, Transition,
};
