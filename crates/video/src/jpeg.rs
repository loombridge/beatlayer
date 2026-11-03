//! Motion JPEG in Rust, the encoder every computer has (the export spec,
//! R47): each frame a baseline JPEG of its own, three components at full
//! resolution, the standard tables of the JPEG standard's Annex K scaled by
//! the quality.

use crate::decoder::Frame;
use crate::encoder::{CodedFrame, SampleEntry, VideoEncoder};
use crate::error::VideoError;

/// The standard luminance and chrominance quantisation tables, in natural
/// order (Annex K, tables K.1 and K.2).
const LUMA: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
const CHROMA: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

/// Natural position of the `i`th coefficient in zigzag order.
const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// The standard luminance Huffman tables (K.3.3.1, K.3.3.2): code counts by
/// length, then the symbols. All three components use them.
const DC_COUNTS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_COUNTS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_VALUES: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

/// Huffman codes by symbol: the code and its length in bits.
fn codes(counts: &[u8; 16], values: &[u8]) -> [(u16, u8); 256] {
    let mut table = [(0u16, 0u8); 256];
    let (mut code, mut k) = (0u16, 0);
    for (length, &count) in counts.iter().enumerate() {
        for _ in 0..count {
            table[usize::from(values[k])] = (code, length as u8 + 1);
            code += 1;
            k += 1;
        }
        code <<= 1;
    }
    table
}

/// Writes bits most significant first, stuffing a zero byte after 0xFF.
struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, code: u16, length: u8) {
        self.acc = self.acc << length | u32::from(code);
        self.n += u32::from(length);
        while self.n >= 8 {
            let byte = (self.acc >> (self.n - 8)) as u8;
            self.out.push(byte);
            if byte == 0xff {
                self.out.push(0);
            }
            self.n -= 8;
        }
        self.acc &= (1 << self.n) - 1;
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.put((1 << (8 - self.n)) - 1, (8 - self.n) as u8);
        }
        self.out
    }
}

/// A quantisation table scaled by a quality from 1 to 100 as the Independent
/// JPEG Group's library scales it.
fn scaled(table: &[u8; 64], quality: u8) -> [u8; 64] {
    let quality = i32::from(quality.clamp(1, 100));
    let scale = if quality < 50 {
        5000 / quality
    } else {
        200 - quality * 2
    };
    table.map(|q| ((i32::from(q) * scale + 50) / 100).clamp(1, 255) as u8)
}

/// Encodes frames as Motion JPEG.
pub(crate) struct MotionJpeg {
    luma: [u8; 64],
    chroma: [u8; 64],
    dc: [(u16, u8); 256],
    ac: [(u16, u8); 256],
    /// `cosine[x][u]`: the DCT's basis, with its scale folded in.
    cosine: [[f32; 8]; 8],
}

impl MotionJpeg {
    pub(crate) fn new(quality: u8) -> Self {
        let mut cosine = [[0.0; 8]; 8];
        for (x, row) in cosine.iter_mut().enumerate() {
            for (u, value) in row.iter_mut().enumerate() {
                let scale = if u == 0 { 0.5f32.sqrt() } else { 1.0 } * 0.5;
                *value =
                    scale * ((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI / 16.0).cos();
            }
        }
        Self {
            luma: scaled(&LUMA, quality),
            chroma: scaled(&CHROMA, quality),
            dc: codes(&DC_COUNTS, &DC_VALUES),
            ac: codes(&AC_COUNTS, &AC_VALUES),
            cosine,
        }
    }

    /// The 8 by 8 block's DCT, quantised, in zigzag order.
    fn transform(&self, block: &[f32; 64], quant: &[u8; 64]) -> [i32; 64] {
        let mut rows = [0.0f32; 64];
        for y in 0..8 {
            for u in 0..8 {
                rows[y * 8 + u] = (0..8).map(|x| block[y * 8 + x] * self.cosine[x][u]).sum();
            }
        }
        let mut out = [0; 64];
        for (i, &natural) in ZIGZAG.iter().enumerate() {
            let (v, u) = (usize::from(natural) / 8, usize::from(natural) % 8);
            let sum: f32 = (0..8).map(|y| rows[y * 8 + u] * self.cosine[y][v]).sum();
            out[i] = (sum / f32::from(quant[usize::from(natural)])).round() as i32;
        }
        out
    }

    fn put_block(&self, bits: &mut Bits, coefficients: &[i32; 64], previous: &mut i32) {
        let magnitude = |v: i32| 32 - v.unsigned_abs().leading_zeros();
        // A negative value is sent as the low bits of its one's complement.
        let value_bits = |v: i32, size: u32| {
            let v = if v < 0 { v - 1 } else { v };
            (v & ((1 << size) - 1)) as u16
        };
        let diff = coefficients[0] - *previous;
        *previous = coefficients[0];
        let size = magnitude(diff);
        let (code, length) = self.dc[size as usize];
        bits.put(code, length);
        if size > 0 {
            bits.put(value_bits(diff, size), size as u8);
        }
        let mut run = 0;
        for &value in &coefficients[1..] {
            if value == 0 {
                run += 1;
                continue;
            }
            while run > 15 {
                let (code, length) = self.ac[0xf0];
                bits.put(code, length);
                run -= 16;
            }
            let size = magnitude(value);
            let (code, length) = self.ac[(run << 4 | size) as usize];
            bits.put(code, length);
            bits.put(value_bits(value, size), size as u8);
            run = 0;
        }
        if run > 0 {
            let (code, length) = self.ac[0];
            bits.put(code, length);
        }
    }

    fn jpeg(&self, frame: &Frame) -> Vec<u8> {
        let (width, height) = (frame.width() as usize, frame.height() as usize);
        let rgba = frame.rgba();
        let mut bits = Bits {
            out: Vec::with_capacity(width * height / 4),
            acc: 0,
            n: 0,
        };
        let mut previous = [0; 3];
        for top in (0..height).step_by(8) {
            for left in (0..width).step_by(8) {
                let mut blocks = [[0.0f32; 64]; 3];
                for y in 0..8 {
                    for x in 0..8 {
                        // The edge's pixel fills the part of a block past it.
                        let at =
                            ((top + y).min(height - 1) * width + (left + x).min(width - 1)) * 4;
                        let (r, g, b) = (
                            f32::from(rgba[at]),
                            f32::from(rgba[at + 1]),
                            f32::from(rgba[at + 2]),
                        );
                        blocks[0][y * 8 + x] = 0.299 * r + 0.587 * g + 0.114 * b - 128.0;
                        blocks[1][y * 8 + x] = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
                        blocks[2][y * 8 + x] = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
                    }
                }
                for (c, block) in blocks.iter().enumerate() {
                    let quant = if c == 0 { &self.luma } else { &self.chroma };
                    let coefficients = self.transform(block, quant);
                    self.put_block(&mut bits, &coefficients, &mut previous[c]);
                }
            }
        }
        let segment = |out: &mut Vec<u8>, marker: u8, body: &[u8]| {
            out.extend_from_slice(&[0xff, marker]);
            out.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
            out.extend_from_slice(body);
        };
        let mut out = vec![0xff, 0xd8];
        segment(
            &mut out,
            0xe0,
            b"JFIF\0\x01\x01\x00\x00\x01\x00\x01\x00\x00",
        );
        for (id, table) in [(0u8, &self.luma), (1, &self.chroma)] {
            let mut body = vec![id];
            body.extend(ZIGZAG.iter().map(|&i| table[usize::from(i)]));
            segment(&mut out, 0xdb, &body);
        }
        let mut body = vec![8];
        body.extend_from_slice(&(height as u16).to_be_bytes());
        body.extend_from_slice(&(width as u16).to_be_bytes());
        body.push(3);
        for (id, table) in [(1u8, 0u8), (2, 1), (3, 1)] {
            body.extend_from_slice(&[id, 0x11, table]);
        }
        segment(&mut out, 0xc0, &body);
        let mut body = vec![0x00];
        body.extend_from_slice(&DC_COUNTS);
        body.extend_from_slice(&DC_VALUES);
        segment(&mut out, 0xc4, &body);
        let mut body = vec![0x10];
        body.extend_from_slice(&AC_COUNTS);
        body.extend_from_slice(&AC_VALUES);
        segment(&mut out, 0xc4, &body);
        segment(&mut out, 0xda, &[3, 1, 0x00, 2, 0x00, 3, 0x00, 0, 63, 0]);
        out.extend_from_slice(&bits.finish());
        out.extend_from_slice(&[0xff, 0xd9]);
        out
    }
}

impl VideoEncoder for MotionJpeg {
    fn encode(&mut self, frame: &Frame, out: &mut Vec<CodedFrame>) -> Result<(), VideoError> {
        out.push(CodedFrame {
            data: self.jpeg(frame),
            sync: true,
        });
        Ok(())
    }

    fn finish(&mut self, _out: &mut Vec<CodedFrame>) -> Result<(), VideoError> {
        Ok(())
    }

    fn entry(&self) -> Result<SampleEntry, VideoError> {
        Ok(SampleEntry {
            fourcc: *b"jpeg",
            extra: Vec::new(),
        })
    }
}
