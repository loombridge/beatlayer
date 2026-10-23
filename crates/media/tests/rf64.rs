//! RF64 and BW64 files over 4 GB.
//!
//! The first tests read a virtual file: a reader that makes up a 4.8 GB BW64
//! or RF64 file on the fly, so they need no disk space and run everywhere.
//! The last writes a real sparse file over 4 GB on macOS and Linux, where a
//! file with a hole takes no space.

use std::io::{self, Read, Seek, SeekFrom};

use bl_media::{Codec, Container, Decoder};

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
/// 4.8 GB of 16-bit stereo: past the 4 GiB a RIFF size can hold.
const FRAMES: u64 = 1_200_000_000;
const BLOCK: u64 = 4;

/// The 16-bit sample at a frame and channel: a hash, so every position
/// differs and a read from the wrong offset can't match by chance.
fn sample(frame: u64, channel: u64) -> i16 {
    let x = frame.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ channel.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    (x >> 48) as i16
}

/// The header of a 64-bit WAVE file: magic, ds64, a Broadcast Wave `bext`
/// chunk, fmt, and the start of data.
fn header(magic: &[u8; 4], data_bytes: u64) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&CHANNELS.to_le_bytes());
    fmt.extend_from_slice(&RATE.to_le_bytes());
    fmt.extend_from_slice(&(RATE * BLOCK as u32).to_le_bytes());
    fmt.extend_from_slice(&(BLOCK as u16).to_le_bytes());
    fmt.extend_from_slice(&16u16.to_le_bytes());

    let bext = vec![0u8; 602];
    let chunks_len = 8 + 28 + 8 + bext.len() as u64 + 8 + fmt.len() as u64 + 8;
    let riff_size = 4 + chunks_len + data_bytes;

    let mut out = Vec::new();
    out.extend_from_slice(magic);
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"ds64");
    out.extend_from_slice(&28u32.to_le_bytes());
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(&data_bytes.to_le_bytes());
    out.extend_from_slice(&(data_bytes / BLOCK).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(b"bext");
    out.extend_from_slice(&(bext.len() as u32).to_le_bytes());
    out.extend_from_slice(&bext);
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    out.extend_from_slice(&fmt);
    out.extend_from_slice(b"data");
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out
}

/// A 64-bit WAVE file made up as it's read.
struct VirtualFile {
    header: Vec<u8>,
    /// The bytes of audio the file really has, which may be fewer than its
    /// header says.
    data_bytes: u64,
    position: u64,
}

impl VirtualFile {
    fn new(magic: &[u8; 4], declared_frames: u64, actual_frames: u64) -> Self {
        Self {
            header: header(magic, declared_frames * BLOCK),
            data_bytes: actual_frames * BLOCK,
            position: 0,
        }
    }

    fn len(&self) -> u64 {
        self.header.len() as u64 + self.data_bytes
    }

    fn byte(&self, at: u64) -> u8 {
        let header_len = self.header.len() as u64;
        if at < header_len {
            return self.header[at as usize];
        }
        let offset = at - header_len;
        let frame = offset / BLOCK;
        let channel = offset % BLOCK / 2;
        sample(frame, channel).to_le_bytes()[(offset % 2) as usize]
    }
}

impl Read for VirtualFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = (self.len().saturating_sub(self.position)).min(buf.len() as u64) as usize;
        for (i, byte) in buf[..n].iter_mut().enumerate() {
            *byte = self.byte(self.position + i as u64);
        }
        self.position += n as u64;
        Ok(n)
    }
}

impl Seek for VirtualFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.position.checked_add_signed(d),
        };
        self.position = target.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.position)
    }
}

/// Reads up to `frames` frames and checks each against [`sample`].
fn read_and_check(decoder: &mut Decoder, frames: usize) -> usize {
    let start = decoder.position();
    let mut left = vec![0.0f32; frames];
    let mut right = vec![0.0f32; frames];
    let n = decoder.read(&mut [&mut left[..], &mut right[..]]).unwrap();
    for i in 0..n {
        let frame = start + i as u64;
        assert_eq!(
            left[i],
            f32::from(sample(frame, 0)) / 32_768.0,
            "frame {frame}"
        );
        assert_eq!(
            right[i],
            f32::from(sample(frame, 1)) / 32_768.0,
            "frame {frame}"
        );
    }
    n
}

#[test]
fn reads_bw64_and_rf64_over_4_gb() {
    // Spec: audio-import R2
    for magic in [b"BW64", b"RF64"] {
        let file = VirtualFile::new(magic, FRAMES, FRAMES);
        assert!(file.len() > 1 << 32, "the virtual file must pass 4 GiB");
        let mut decoder = Decoder::from_reader(file, Some("wav")).unwrap();
        let info = *decoder.info();
        assert_eq!(info.container, Container::Rf64);
        assert_eq!(info.codec, Codec::Pcm);
        assert_eq!(info.sample_rate, RATE);
        assert_eq!(info.channels, CHANNELS);
        assert_eq!(info.frames, Some(FRAMES));
        assert_eq!(info.bits_per_sample, Some(16));

        // The start, then frames whose bytes lie past 4 GiB.
        assert_eq!(read_and_check(&mut decoder, 4096), 4096);
        let past_4_gib = (1u64 << 32) / BLOCK + 12_345;
        decoder.seek(past_4_gib).unwrap();
        assert_eq!(read_and_check(&mut decoder, 4096), 4096);
        decoder.seek(FRAMES - 4096 - 10).unwrap();
        assert_eq!(read_and_check(&mut decoder, 4096), 4096);

        // The end: the last ten frames, then nothing.
        assert_eq!(read_and_check(&mut decoder, 4096), 10);
        assert_eq!(read_and_check(&mut decoder, 4096), 0);
        decoder.seek(FRAMES + 1).unwrap();
        assert_eq!(read_and_check(&mut decoder, 4096), 0);
    }
}

#[test]
fn a_file_shorter_than_its_header_says_keeps_what_it_has() {
    // Spec: audio-import R8
    let file = VirtualFile::new(b"RF64", FRAMES, 5_000);
    let mut decoder = Decoder::from_reader(file, None).unwrap();
    assert_eq!(decoder.info().frames, Some(5_000));
    assert_eq!(read_and_check(&mut decoder, 8_192), 5_000);
    assert_eq!(read_and_check(&mut decoder, 8_192), 0);
}

/// A real file over 4 GB, on file systems where a hole costs nothing.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn opens_a_sparse_file_over_4_gb_from_disk() {
    // Spec: audio-import R2
    use std::io::Write;

    let dir = crate::common::TempDir::new("rf64");
    let path = dir.path().join("long.wav");
    let header = header(b"BW64", FRAMES * BLOCK);
    let tail_frames = 50_000u64;
    let tail_start = FRAMES - tail_frames;

    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(&header).unwrap();
    // Leave a hole up to the tail, then write the tail's samples.
    file.set_len(header.len() as u64 + tail_start * BLOCK)
        .unwrap();
    file.seek(SeekFrom::End(0)).unwrap();
    let mut tail = Vec::with_capacity((tail_frames * BLOCK) as usize);
    for frame in tail_start..FRAMES {
        tail.extend_from_slice(&sample(frame, 0).to_le_bytes());
        tail.extend_from_slice(&sample(frame, 1).to_le_bytes());
    }
    file.write_all(&tail).unwrap();
    drop(file);
    assert!(std::fs::metadata(&path).unwrap().len() > 1 << 32);

    let mut decoder = Decoder::open(&path).unwrap();
    assert_eq!(decoder.info().frames, Some(FRAMES));
    decoder.seek(tail_start + 1_000).unwrap();
    assert_eq!(read_and_check(&mut decoder, 8_192), 8_192);
    decoder.seek(FRAMES - 100).unwrap();
    assert_eq!(read_and_check(&mut decoder, 8_192), 100);
}
