//! A WAV writer for files the streaming tests generate, written out a frame
//! at a time so long files never sit in memory.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// How samples are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// 16-bit integer: a value decodes to `round(value × 32768) / 32768`.
    Pcm16,
    /// 24-bit integer.
    Pcm24,
    /// 32-bit float, exactly.
    Float32,
}

impl Format {
    fn bytes(self) -> u16 {
        match self {
            Self::Pcm16 => 2,
            Self::Pcm24 => 3,
            Self::Float32 => 4,
        }
    }
}

/// Writes a WAV file of `frames` frames, taking each sample from
/// `sample(frame, channel)`.
pub fn write(
    path: &Path,
    rate: u32,
    channels: u16,
    frames: u64,
    format: Format,
    mut sample: impl FnMut(u64, u16) -> f32,
) {
    let width = format.bytes();
    let data_len = frames * u64::from(channels) * u64::from(width);
    let mut out = BufWriter::with_capacity(1 << 20, File::create(path).unwrap());
    let tag: u16 = if format == Format::Float32 { 3 } else { 1 };
    let block_align = channels * width;
    out.write_all(b"RIFF").unwrap();
    out.write_all(&u32::try_from(36 + data_len).unwrap().to_le_bytes())
        .unwrap();
    out.write_all(b"WAVEfmt ").unwrap();
    out.write_all(&16u32.to_le_bytes()).unwrap();
    out.write_all(&tag.to_le_bytes()).unwrap();
    out.write_all(&channels.to_le_bytes()).unwrap();
    out.write_all(&rate.to_le_bytes()).unwrap();
    out.write_all(&(rate * u32::from(block_align)).to_le_bytes())
        .unwrap();
    out.write_all(&block_align.to_le_bytes()).unwrap();
    out.write_all(&(width * 8).to_le_bytes()).unwrap();
    out.write_all(b"data").unwrap();
    out.write_all(&u32::try_from(data_len).unwrap().to_le_bytes())
        .unwrap();
    for frame in 0..frames {
        for channel in 0..channels {
            let value = sample(frame, channel);
            match format {
                Format::Pcm16 => {
                    let code = (value * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16;
                    out.write_all(&code.to_le_bytes()).unwrap();
                }
                Format::Pcm24 => {
                    let code = (value * 8_388_608.0)
                        .round()
                        .clamp(-8_388_608.0, 8_388_607.0) as i32;
                    out.write_all(&code.to_le_bytes()[..3]).unwrap();
                }
                Format::Float32 => out.write_all(&value.to_le_bytes()).unwrap(),
            }
        }
    }
    out.flush().unwrap();
}

/// A deterministic noise sample in [-1, 1) that's exact in 16-bit: a hash of
/// the frame, channel, and seed, as a multiple of 1/32768.
pub fn noise(seed: u64, frame: u64, channel: u16) -> f32 {
    let mut x = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(frame.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add(u64::from(channel).wrapping_mul(0x94D0_49BB_1331_11EB));
    x ^= x >> 31;
    x = x.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    x ^= x >> 29;
    f32::from((x >> 48) as u16 as i16) / 32_768.0
}
