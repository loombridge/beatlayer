//! The peak file format, version 1.
//!
//! A peak file is a header followed by the data block exactly as
//! [`Peaks::data`] holds it, so loading one is a read and a check, and the
//! block goes to the GPU as it is. All numbers are little-endian.
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 8 | Magic: `BLPEAKS` and a zero byte |
//! | 8 | 4 | Format version, `u32`: 1 |
//! | 12 | 4 | Header length in bytes, `u32`: where the data block starts. A multiple of 16. |
//! | 16 | 32 | The source's content hash (BLAKE3 of the media file's bytes) |
//! | 48 | 8 | Frames in the source, `u64` |
//! | 56 | 4 | Sample rate, `u32` |
//! | 60 | 2 | Channels, `u16` |
//! | 62 | 2 | Levels, `u16` |
//! | 64 | 4 | Frames per peak at level 0, `u32`: 256 |
//! | 68 | 4 | Peaks merged per level, `u32`: 4 |
//! | 72 | 8 | Data block length in bytes, `u64` |
//! | 80 | 32 | Checksum: the BLAKE3 hash of the whole file, with these 32 bytes read as zero |
//! | 112 | 16 | Reserved, zero |
//! | 128 | 8 per run | The runs, level-major: offset and length in peaks, two `u32`s each |
//! | header length | data block length | The data block |
//!
//! A reader rejects a file whose magic, version, constants, source hash, run
//! table, lengths, or checksum don't match what it expects, and the cache then
//! builds the peaks again. The version goes up whenever the layout or the way
//! peaks are computed changes, so stale files are rebuilt rather than misread.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{FRAMES_PER_PEAK, Layout, MIP_FACTOR, PeakRun, Peaks};
use crate::error::MediaError;
use crate::hash::ContentHash;

/// The file name extension of peak files, without the dot.
pub const FILE_EXTENSION: &str = "blpeaks";

/// The peak file format version this build reads and writes.
pub const FORMAT_VERSION: u32 = 1;

const MAGIC: [u8; 8] = *b"BLPEAKS\0";
const FIXED_HEADER: usize = 128;
const CHECKSUM: std::ops::Range<usize> = 80..112;

impl Peaks {
    /// The peaks as a peak file: header, then data block. `source` is the
    /// content hash of the media they were built from.
    #[must_use]
    pub fn to_file_bytes(&self, source: ContentHash) -> Vec<u8> {
        let mut bytes = self.header(source);
        bytes.extend_from_slice(&self.data);
        bytes
    }

    /// Reads a peak file, checking it's whole, current, and built from the
    /// media `source` names.
    ///
    /// # Errors
    ///
    /// [`MediaError::Malformed`] for a file that isn't a whole version-1 peak
    /// file of `source`.
    pub fn from_file_bytes(bytes: &[u8], source: ContentHash) -> Result<Self, MediaError> {
        parse(bytes, source).map_err(|why| MediaError::Malformed(format!("peak file: {why}")))
    }

    /// Reads a peak file from disk, as [`Peaks::from_file_bytes`] does.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read, or as for [`Peaks::from_file_bytes`].
    pub fn read_file(path: impl AsRef<Path>, source: ContentHash) -> Result<Self, MediaError> {
        Self::from_file_bytes(&fs::read(path)?, source)
    }

    /// Writes a peak file. It's written beside its final path under a
    /// temporary name and then renamed into place, so a reader never sees a
    /// file half-written, and a write stopped part-way leaves no file at
    /// `path`.
    ///
    /// # Errors
    ///
    /// Fails if the folder can't be created or the file can't be written.
    pub fn write_file(&self, path: impl AsRef<Path>, source: ContentHash) -> io::Result<()> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = path.as_ref();
        let folder = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(folder)?;
        let name = path
            .file_name()
            .map_or_else(|| "peaks".into(), |n| n.to_string_lossy().into_owned());
        let temporary = folder.join(format!(
            ".{name}.{}-{}.tmp",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let written = (|| {
            let mut file = File::create(&temporary)?;
            file.write_all(&self.header(source))?;
            file.write_all(&self.data)?;
            drop(file);
            fs::rename(&temporary, path)
        })();
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        written
    }

    fn header(&self, source: ContentHash) -> Vec<u8> {
        let header_len = header_len(self.runs.len());
        let mut h = Vec::with_capacity(header_len + self.data.len());
        h.extend_from_slice(&MAGIC);
        h.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        h.extend_from_slice(&(header_len as u32).to_le_bytes());
        h.extend_from_slice(source.as_bytes());
        h.extend_from_slice(&self.frames.to_le_bytes());
        h.extend_from_slice(&self.sample_rate.to_le_bytes());
        h.extend_from_slice(&self.channels.to_le_bytes());
        h.extend_from_slice(&(self.level_count() as u16).to_le_bytes());
        h.extend_from_slice(&(FRAMES_PER_PEAK as u32).to_le_bytes());
        h.extend_from_slice(&(MIP_FACTOR as u32).to_le_bytes());
        h.extend_from_slice(&(self.data.len() as u64).to_le_bytes());
        h.resize(FIXED_HEADER, 0);
        for run in &self.runs {
            h.extend_from_slice(&run.offset.to_le_bytes());
            h.extend_from_slice(&run.len.to_le_bytes());
        }
        h.resize(header_len, 0);
        let sum = checksum(&h, &self.data);
        h[CHECKSUM].copy_from_slice(&sum);
        h
    }
}

/// The checksum of a file: its header, with the checksum field read as
/// zero, then its data.
fn checksum(header: &[u8], data: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&header[..CHECKSUM.start]);
    hasher.update(&[0; 32]);
    hasher.update(&header[CHECKSUM.end..]);
    hasher.update(data);
    *hasher.finalize().as_bytes()
}

/// The header's length for a number of runs: the fixed part and the run
/// table, padded to 16 bytes.
fn header_len(runs: usize) -> usize {
    (FIXED_HEADER + 8 * runs).next_multiple_of(16)
}

fn parse(bytes: &[u8], source: ContentHash) -> Result<Peaks, String> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(8)? != MAGIC {
        return Err("not a peak file".into());
    }
    let version = r.u32()?;
    if version != FORMAT_VERSION {
        return Err(format!("version {version}, not {FORMAT_VERSION}"));
    }
    let header = r.u32()? as usize;
    if r.take(32)? != source.as_bytes() {
        return Err("built from other media".into());
    }
    let frames = r.u64()?;
    let sample_rate = r.u32()?;
    let channels = r.u16()?;
    let levels = usize::from(r.u16()?);
    let frames_per_peak = u64::from(r.u32()?);
    let factor = u64::from(r.u32()?);
    let data_len = r.u64()?;
    let sum = r.take(32)?;
    if sample_rate == 0 || channels == 0 {
        return Err("no sample rate or no channels".into());
    }
    if frames_per_peak != FRAMES_PER_PEAK || factor != MIP_FACTOR {
        return Err(format!(
            "{frames_per_peak} frames per peak, merging {factor}"
        ));
    }
    let min_data = frames
        .div_ceil(FRAMES_PER_PEAK)
        .checked_mul(u64::from(channels))
        .and_then(|p| p.checked_mul(2));
    if min_data.is_none_or(|needed| needed > bytes.len() as u64) {
        return Err("the file is too short for its frame count".into());
    }
    let layout = Layout::new(frames, channels).map_err(|e| e.to_string())?;
    if levels != layout.levels
        || header != header_len(layout.runs.len())
        || data_len != layout.data_len as u64
    {
        return Err("the header's sizes don't fit its length and channels".into());
    }
    r.at = FIXED_HEADER;
    for expected in &layout.runs {
        let run = PeakRun {
            offset: r.u32()?,
            len: r.u32()?,
        };
        if run != *expected {
            return Err("the run table doesn't fit the length and channels".into());
        }
    }
    let data = bytes.get(header..).ok_or("cut short before the data")?;
    if data.len() as u64 != data_len {
        return Err(format!("{} data bytes, not {data_len}", data.len()));
    }
    if checksum(&bytes[..header], data) != sum {
        return Err("the checksum doesn't match".into());
    }
    Ok(Peaks {
        sample_rate,
        channels,
        frames,
        runs: layout.runs.into_boxed_slice(),
        data: data.into(),
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let slice = self
            .bytes
            .get(self.at..self.at + n)
            .ok_or("cut short in the header")?;
        self.at += n;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| "")?,
        ))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| "")?,
        ))
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| "")?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_peaks() -> Peaks {
        // 1,280 frames: 5, 2, and 1 peaks per channel.
        let level0 = |sign: i8| -> Vec<[u8; 2]> {
            (0..5i8)
                .map(|i| [(-i * sign) as u8, (i * 20) as u8])
                .collect()
        };
        Peaks::assemble(44_100, 1_280, &[level0(1), level0(-1)]).unwrap()
    }

    #[test]
    fn the_header_is_laid_out_as_documented() {
        // Spec: waveform-peaks R8
        let peaks = sample_peaks();
        let source = ContentHash::of_bytes(b"source");
        let bytes = peaks.to_file_bytes(source);
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(&bytes[0..8], b"BLPEAKS\0");
        assert_eq!(u32_at(8), 1);
        // 128 + 6 runs × 8 = 176, already a multiple of 16.
        assert_eq!(u32_at(12), 176);
        assert_eq!(&bytes[16..48], source.as_bytes());
        assert_eq!(u64::from_le_bytes(bytes[48..56].try_into().unwrap()), 1_280);
        assert_eq!(u32_at(56), 44_100);
        assert_eq!(u16::from_le_bytes([bytes[60], bytes[61]]), 2);
        assert_eq!(u16::from_le_bytes([bytes[62], bytes[63]]), 3);
        assert_eq!(u32_at(64), 256);
        assert_eq!(u32_at(68), 4);
        assert_eq!(u64::from_le_bytes(bytes[72..80].try_into().unwrap()), 40);
        let mut zeroed = bytes.clone();
        zeroed[80..112].fill(0);
        assert_eq!(&bytes[80..112], blake3::hash(&zeroed).as_bytes());
        assert!(bytes[112..128].iter().all(|&b| b == 0));
        // The second run: level 0, channel 1, at peak 6, 5 long.
        assert_eq!((u32_at(136), u32_at(140)), (6, 5));
        assert_eq!(&bytes[176..], peaks.data());
    }

    #[test]
    fn round_trips() {
        let peaks = sample_peaks();
        let source = ContentHash::of_bytes(b"source");
        let bytes = peaks.to_file_bytes(source);
        assert_eq!(Peaks::from_file_bytes(&bytes, source).unwrap(), peaks);
    }

    #[test]
    fn rejects_anything_not_whole_and_current() {
        // Spec: waveform-peaks R4
        let peaks = sample_peaks();
        let source = ContentHash::of_bytes(b"source");
        let good = peaks.to_file_bytes(source);
        let rejects = |bytes: &[u8], hash: ContentHash| {
            assert!(matches!(
                Peaks::from_file_bytes(bytes, hash),
                Err(MediaError::Malformed(_))
            ));
        };
        // Other media's peaks.
        rejects(&good, ContentHash::of_bytes(b"other"));
        // Cut short anywhere.
        for len in [0, 7, 60, 127, 150, 176, good.len() - 1] {
            rejects(&good[..len], source);
        }
        // Too long.
        let mut long = good.clone();
        long.push(0);
        rejects(&long, source);
        // Any single byte changed, in the header or the data.
        for at in 0..good.len() {
            let mut bad = good.clone();
            bad[at] ^= 0x10;
            rejects(&bad, source);
        }
    }

    #[test]
    fn writes_atomically_and_reads_back() {
        let dir = std::env::temp_dir().join(format!(
            "bl-media-peak-file-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = dir.join("ab").join("peaks.blpeaks");
        let peaks = sample_peaks();
        let source = ContentHash::of_bytes(b"source");
        peaks.write_file(&path, source).unwrap();
        assert_eq!(Peaks::read_file(&path, source).unwrap(), peaks);
        // Nothing else is left in the folder.
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["peaks.blpeaks"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_a_header_whose_frame_count_the_file_cannot_hold() {
        let peaks = sample_peaks();
        let source = ContentHash::of_bytes(b"source");
        let mut bytes = peaks.to_file_bytes(source);
        let huge_frames: u64 = 1_000_000_000;
        bytes[48..56].copy_from_slice(&huge_frames.to_le_bytes());
        let err = Peaks::from_file_bytes(&bytes, source).unwrap_err();
        assert_eq!(
            err.to_string(),
            "damaged or malformed media: peak file: the file is too short for its frame count"
        );
    }
}
