//! RF64 and BW64: WAV with 64-bit sizes, for files over 4 GB.
//!
//! RF64 (EBU Tech 3306, 2009) and BW64 (ITU-R BS.2088) replace the RIFF
//! header's 32-bit sizes with `0xFFFFFFFF` and keep the real sizes in a `ds64`
//! chunk. Symphonia 0.6 doesn't read either, so this module does, for the
//! PCM and IEEE float encodings that such long files carry in practice. A
//! Broadcast Wave file over 4 GB is one of these; under 4 GB it's a RIFF WAV
//! with a `bext` chunk, which symphonia reads.

use std::io::{self, Read, Seek, SeekFrom};

use crate::error::MediaError;
use crate::limits;

/// The marker that sizes are in the `ds64` chunk.
const SIZE_IN_DS64: u32 = 0xFFFF_FFFF;

/// The tail shared by the PCM and float `KSDATAFORMAT_SUBTYPE` GUIDs.
const SUBTYPE_TAIL: [u8; 14] = [
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

/// Whether a file starting with these four bytes is RF64 or BW64.
pub(crate) fn is_rf64(magic: &[u8]) -> bool {
    magic == b"RF64" || magic == b"BW64"
}

/// How samples are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    /// Integers of this many bytes: unsigned for one byte, else signed
    /// little-endian.
    Int(usize),
    /// IEEE float of this many bytes, little-endian.
    Float(usize),
}

/// The stream's shape, from its `fmt ` and `ds64` chunks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rf64Format {
    pub sample_rate: u32,
    pub channels: u16,
    /// The significant bits per sample (fewer than the container for, say,
    /// 20-bit audio in 24-bit containers).
    pub bits_per_sample: u32,
    pub frames: u64,
    /// Whether the samples are floating point.
    pub float: bool,
}

/// Reads PCM frames from an RF64 or BW64 stream.
pub(crate) struct Rf64Reader<R> {
    reader: R,
    format: Rf64Format,
    encoding: Encoding,
    block_align: usize,
    data_start: u64,
    position: u64,
    scratch: Vec<u8>,
}

impl<R: Read + Seek> Rf64Reader<R> {
    /// Parses the header and stops at the start of the samples.
    pub fn open(mut reader: R) -> Result<Self, MediaError> {
        let length = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;

        let mut header = [0; 12];
        reader.read_exact(&mut header)?;
        if !is_rf64(&header[0..4]) || &header[8..12] != b"WAVE" {
            return Err(malformed("not an RF64 or BW64 WAVE file"));
        }

        let mut ds64: Option<Ds64> = None;
        let mut fmt: Option<Fmt> = None;
        let mut position = 12u64;
        loop {
            let mut chunk = [0; 8];
            if read_or_eof(&mut reader, &mut chunk)? {
                return Err(malformed("no data chunk"));
            }
            position += 8;
            let id: [u8; 4] = chunk[0..4].try_into().unwrap_or_default();
            let size32 = u32::from_le_bytes(chunk[4..8].try_into().unwrap_or_default());

            let size = match &id {
                b"ds64" => {
                    let body = read_body(&mut reader, u64::from(size32), 4096)?;
                    ds64 = Some(Ds64::parse(&body)?);
                    u64::from(size32)
                }
                b"fmt " => {
                    let body = read_body(&mut reader, u64::from(size32), 4096)?;
                    fmt = Some(Fmt::parse(&body)?);
                    u64::from(size32)
                }
                b"data" => {
                    let size = if size32 == SIZE_IN_DS64 {
                        ds64.as_ref()
                            .ok_or_else(|| malformed("data size is in a missing ds64 chunk"))?
                            .data_size
                    } else {
                        u64::from(size32)
                    };
                    let fmt = fmt.ok_or_else(|| malformed("data before fmt"))?;
                    // A file cut short keeps the frames it has.
                    let available = size.min(length.saturating_sub(position));
                    let frames = available / fmt.block_align as u64;
                    limits::check_header(u64::from(fmt.channels), fmt.sample_rate, Some(frames))?;
                    return Ok(Self {
                        reader,
                        format: Rf64Format {
                            sample_rate: fmt.sample_rate,
                            channels: fmt.channels,
                            bits_per_sample: fmt.valid_bits,
                            frames,
                            float: matches!(fmt.encoding, Encoding::Float(_)),
                        },
                        encoding: fmt.encoding,
                        block_align: fmt.block_align,
                        data_start: position,
                        position: 0,
                        scratch: Vec::new(),
                    });
                }
                _ if size32 == SIZE_IN_DS64 => ds64
                    .as_ref()
                    .and_then(|d| d.table_size(id))
                    .ok_or_else(|| malformed("a chunk's size is missing from ds64"))?,
                _ => u64::from(size32),
            };
            // Chunks are padded to an even length.
            position = size
                .checked_add(size % 2)
                .and_then(|padded| position.checked_add(padded))
                .ok_or_else(|| malformed("chunk too large"))?;
            reader.seek(SeekFrom::Start(position))?;
        }
    }

    pub fn format(&self) -> &Rf64Format {
        &self.format
    }

    /// Reads up to the shortest output slice's length in frames into the
    /// planar outputs, one per channel, and returns the frames read: zero at
    /// the end.
    pub fn read(&mut self, out: &mut [&mut [f32]]) -> Result<usize, MediaError> {
        let channels = usize::from(self.format.channels);
        if out.len() != channels {
            return Err(MediaError::Malformed(format!(
                "{} output channels for {channels}-channel audio",
                out.len()
            )));
        }
        let wanted = out.iter().map(|c| c.len()).min().unwrap_or(0) as u64;
        let frames = wanted.min(self.format.frames - self.position) as usize;
        if frames == 0 {
            return Ok(0);
        }
        let bytes = frames * self.block_align;
        self.scratch.resize(bytes, 0);
        self.reader.read_exact(&mut self.scratch)?;

        let width = self.block_align / channels;
        for (frame, block) in self.scratch.chunks_exact(self.block_align).enumerate() {
            for (channel, sample) in block.chunks_exact(width).enumerate() {
                out[channel][frame] = convert(self.encoding, sample);
            }
        }
        self.position += frames as u64;
        Ok(frames)
    }

    /// Moves to a frame; past the end means at the end.
    pub fn seek(&mut self, frame: u64) -> Result<(), MediaError> {
        let frame = frame.min(self.format.frames);
        let offset = frame
            .checked_mul(self.block_align as u64)
            .and_then(|o| o.checked_add(self.data_start))
            .ok_or_else(|| malformed("seek beyond the file"))?;
        self.reader.seek(SeekFrom::Start(offset))?;
        self.position = frame;
        Ok(())
    }
}

/// Converts one sample to `f32`, with the same scaling symphonia uses for
/// RIFF WAV, so a file reads identically as RIFF or as RF64.
fn convert(encoding: Encoding, bytes: &[u8]) -> f32 {
    match (encoding, bytes) {
        (Encoding::Int(1), &[b]) => f32::from(b) / 128.0 - 1.0,
        (Encoding::Int(2), &[b0, b1]) => f32::from(i16::from_le_bytes([b0, b1])) / 32_768.0,
        (Encoding::Int(3), &[b0, b1, b2]) => {
            (i32::from_le_bytes([0, b0, b1, b2]) >> 8) as f32 / 8_388_608.0
        }
        (Encoding::Int(4), &[b0, b1, b2, b3]) => {
            (f64::from(i32::from_le_bytes([b0, b1, b2, b3])) / 2_147_483_648.0) as f32
        }
        (Encoding::Float(4), &[b0, b1, b2, b3]) => f32::from_le_bytes([b0, b1, b2, b3]),
        (Encoding::Float(8), &[b0, b1, b2, b3, b4, b5, b6, b7]) => {
            f64::from_le_bytes([b0, b1, b2, b3, b4, b5, b6, b7]) as f32
        }
        // `Fmt::parse` admits no other widths.
        _ => 0.0,
    }
}

/// The `ds64` chunk: the 64-bit sizes.
struct Ds64 {
    data_size: u64,
    /// Sizes of other chunks over 4 GB, by chunk ID.
    table: Vec<([u8; 4], u64)>,
}

impl Ds64 {
    fn parse(body: &[u8]) -> Result<Self, MediaError> {
        if body.len() < 28 {
            return Err(malformed("ds64 chunk too short"));
        }
        let data_size = le_u64(&body[8..16]);
        let entries = le_u32(&body[24..28]) as usize;
        let (rows, _) = body[28..].as_chunks::<12>();
        let table = rows
            .iter()
            .take(entries)
            .map(|e| {
                let id: [u8; 4] = e[0..4].try_into().unwrap_or_default();
                (id, le_u64(&e[4..12]))
            })
            .collect();
        Ok(Self { data_size, table })
    }

    fn table_size(&self, id: [u8; 4]) -> Option<u64> {
        self.table.iter().find(|(i, _)| *i == id).map(|&(_, s)| s)
    }
}

/// The `fmt ` chunk.
#[derive(Clone, Copy)]
struct Fmt {
    sample_rate: u32,
    channels: u16,
    block_align: usize,
    valid_bits: u32,
    encoding: Encoding,
}

impl Fmt {
    fn parse(body: &[u8]) -> Result<Self, MediaError> {
        const PCM: u16 = 1;
        const FLOAT: u16 = 3;
        const EXTENSIBLE: u16 = 0xFFFE;

        if body.len() < 16 {
            return Err(malformed("fmt chunk too short"));
        }
        let mut tag = le_u16(&body[0..2]);
        let channels = le_u16(&body[2..4]);
        let sample_rate = le_u32(&body[4..8]);
        let block_align = usize::from(le_u16(&body[12..14]));
        let bits = u32::from(le_u16(&body[14..16]));
        let mut valid_bits = bits;

        if tag == EXTENSIBLE {
            if body.len() < 40 {
                return Err(malformed("extensible fmt chunk too short"));
            }
            let valid = u32::from(le_u16(&body[18..20]));
            if valid != 0 {
                valid_bits = valid;
            }
            if body[26..40] != SUBTYPE_TAIL {
                return Err(MediaError::Unsupported(
                    "RF64 with a non-PCM sub-format".into(),
                ));
            }
            tag = le_u16(&body[24..26]);
        }

        if channels == 0 || sample_rate == 0 || block_align == 0 {
            return Err(malformed(
                "fmt chunk with zero channels, rate, or block size",
            ));
        }
        if block_align % usize::from(channels) != 0 {
            return Err(malformed("block size isn't a whole number of samples"));
        }
        let width = block_align / usize::from(channels);
        let encoding = match (tag, width) {
            (PCM, 1..=4) => Encoding::Int(width),
            (FLOAT, 4 | 8) => Encoding::Float(width),
            (PCM | FLOAT, _) => {
                return Err(MediaError::Unsupported(format!(
                    "RF64 with {width}-byte samples"
                )));
            }
            _ => {
                return Err(MediaError::Unsupported(format!(
                    "RF64 with format tag {tag:#06x}"
                )));
            }
        };
        if valid_bits == 0 || valid_bits > (width as u32) * 8 {
            valid_bits = width as u32 * 8;
        }
        Ok(Self {
            sample_rate,
            channels,
            block_align,
            valid_bits,
            encoding,
        })
    }
}

/// Reads exactly `buf.len()` bytes, or returns `true` at a clean end of file.
fn read_or_eof(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 => return Ok(true),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

fn read_body(reader: &mut impl Read, size: u64, limit: u64) -> Result<Vec<u8>, MediaError> {
    if size > limit {
        return Err(malformed("header chunk too large"));
    }
    let mut body = vec![0; size as usize];
    reader.read_exact(&mut body)?;
    Ok(body)
}

fn le_u16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn le_u64(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

fn malformed(what: &str) -> MediaError {
    MediaError::Malformed(format!("RF64: {what}"))
}
