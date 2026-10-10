//! Content hashes: how the media store names a file by what it holds.
//!
//! A content hash is the BLAKE3 hash of every byte of the file, unkeyed, at
//! its standard 32-byte length. The reasons for the choice:
//!
//! - **Bytes, not decoded samples.** Hashing the file needs no decoder, so it
//!   costs a read at disk speed, and it never changes when a decoder does. A
//!   lossy decoder's output can shift between versions, which would orphan
//!   every clip that named the old hash. The cost is that a change to a file's
//!   tags alone gives it a new hash; relinking by name and length covers that
//!   case later.
//! - **BLAKE3.** It's a cryptographic hash, so two different files never
//!   share a hash in practice and relinking can trust a match. It's also faster
//!   than SHA-256 or SHA-1 on every platform Beatlayer targets thanks to its
//!   SIMD code: 2.4 GB/s on one core of an Apple Silicon Mac (WP-1.1), so an
//!   hour of 24-bit stereo at 48 kHz, about 1 GB, hashes in under half a
//!   second once it's read. Its licence (CC0-1.0 or Apache-2.0) passes
//!   `deny.toml`.
//! - **32 bytes.** The document's `bl_model::ContentHash` is 32 bytes, so a
//!   hash moves between the two without conversion beyond the newtype.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::str::FromStr;

/// The content hash of a media file: the BLAKE3 hash of its bytes, shown as
/// 64 lowercase hex digits.
///
/// It's bit for bit the same value as the document's `bl_model::ContentHash`;
/// convert with `bl_model::ContentHash(*hash.as_bytes())`. `bl-media` doesn't
/// depend on `bl-model`, because the engine side of the architecture reads
/// media without the document.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// A hash from its 32 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The hash's 32 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The hash of a byte slice.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// The hash of everything a reader yields, read to its end.
    ///
    /// # Errors
    ///
    /// Returns any read error other than `Interrupted`.
    pub fn of_reader(mut reader: impl Read) -> io::Result<Self> {
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0; 1 << 16];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    hasher.update(&buffer[..n]);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(Self(*hasher.finalize().as_bytes()))
    }

    /// The hash of a file's contents.
    ///
    /// # Errors
    ///
    /// Returns the error from opening or reading the file.
    pub fn of_file(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::of_reader(File::open(path)?)
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

/// Why a string isn't a [`ContentHash`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseContentHashError;

impl fmt::Display for ParseContentHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a content hash is 64 hex digits")
    }
}

impl std::error::Error for ParseContentHashError {}

impl FromStr for ContentHash {
    type Err = ParseContentHashError;

    /// Parses 64 hex digits, in either case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let digits = s.as_bytes();
        if digits.len() != 64 {
            return Err(ParseContentHashError);
        }
        let mut bytes = [0; 32];
        let (pairs, _) = digits.as_chunks::<2>();
        for (byte, &[high, low]) in bytes.iter_mut().zip(pairs) {
            let high = hex_digit(high).ok_or(ParseContentHashError)?;
            let low = hex_digit(low).ok_or(ParseContentHashError)?;
            *byte = high << 4 | low;
        }
        Ok(Self(bytes))
    }
}

fn hex_digit(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_blake3_reference_vector() {
        // The BLAKE3 hash of the empty input, from the specification's test
        // vectors.
        assert_eq!(
            ContentHash::of_bytes(b"").to_string(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn reader_and_slice_agree_across_buffer_boundaries() {
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let from_reader = ContentHash::of_reader(&bytes[..]).unwrap();
        assert_eq!(from_reader, ContentHash::of_bytes(&bytes));
    }

    #[test]
    fn one_changed_byte_changes_the_hash() {
        let mut bytes = vec![0u8; 4096];
        let before = ContentHash::of_bytes(&bytes);
        bytes[4000] = 1;
        assert_ne!(before, ContentHash::of_bytes(&bytes));
    }

    #[test]
    fn hex_round_trips() {
        let hash = ContentHash::of_bytes(b"beatlayer");
        let text = hash.to_string();
        assert_eq!(text.len(), 64);
        assert_eq!(text.parse::<ContentHash>(), Ok(hash));
        assert_eq!(text.to_uppercase().parse::<ContentHash>(), Ok(hash));
        assert_eq!("00".parse::<ContentHash>(), Err(ParseContentHashError));
        assert_eq!(
            "g".repeat(64).parse::<ContentHash>(),
            Err(ParseContentHashError)
        );
    }
}
