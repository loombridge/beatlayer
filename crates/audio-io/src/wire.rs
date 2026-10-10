//! Raw sample layouts that drivers read and write, such as ASIO's, converted
//! to and from Beatlayer's `f32`. Every function works in place on slices the
//! stream allocated when it opened, so all of it is safe on the audio thread.

/// How one sample is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// 16-bit integer.
    I16,
    /// 24-bit integer packed in 3 bytes.
    I24,
    /// 32-bit integer.
    I32,
    /// 32-bit float.
    F32,
    /// 64-bit float.
    F64,
    /// A `bits`-bit integer in the low bits of a 32-bit container (ASIO's
    /// "32-bit data with 16-, 18-, 20-, or 24-bit alignment").
    I32Low { bits: u32 },
}

/// A sample encoding and its byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WireFormat {
    pub(crate) encoding: Encoding,
    pub(crate) big_endian: bool,
}

impl WireFormat {
    /// Bytes per sample.
    pub(crate) const fn bytes_per_sample(self) -> usize {
        match self.encoding {
            Encoding::I16 => 2,
            Encoding::I24 => 3,
            Encoding::I32 | Encoding::F32 | Encoding::I32Low { .. } => 4,
            Encoding::F64 => 8,
        }
    }

    /// Writes `source` into `target`, which has room for `source.len()`
    /// samples in this format. Samples beyond full scale are clipped.
    pub(crate) fn encode(self, source: &[f32], target: &mut [u8]) {
        let size = self.bytes_per_sample();
        for (&sample, bytes) in source.iter().zip(target.chunks_exact_mut(size)) {
            self.encode_one(sample, bytes);
        }
    }

    /// Reads `target.len()` samples in this format from `source`.
    pub(crate) fn decode(self, source: &[u8], target: &mut [f32]) {
        let size = self.bytes_per_sample();
        for (bytes, sample) in source.chunks_exact(size).zip(target.iter_mut()) {
            *sample = self.decode_one(bytes);
        }
    }

    fn encode_one(self, sample: f32, out: &mut [u8]) {
        match self.encoding {
            Encoding::I16 => {
                let value = to_int(sample, 16) as i16;
                self.put(&value.to_le_bytes(), out);
            }
            Encoding::I24 => self.put(&to_int(sample, 24).to_le_bytes()[..3], out),
            Encoding::I32 => self.put(&to_int(sample, 32).to_le_bytes(), out),
            Encoding::I32Low { bits } => self.put(&to_int(sample, bits).to_le_bytes(), out),
            Encoding::F32 => self.put(&sample.to_le_bytes(), out),
            Encoding::F64 => self.put(&f64::from(sample).to_le_bytes(), out),
        }
    }

    fn decode_one(self, bytes: &[u8]) -> f32 {
        let mut le = [0u8; 8];
        let le = &mut le[..bytes.len()];
        le.copy_from_slice(bytes);
        if self.big_endian {
            le.reverse();
        }
        let word = |le: &[u8]| i32::from_le_bytes([le[0], le[1], le[2], le[3]]);
        match self.encoding {
            Encoding::I16 => from_int(i16::from_le_bytes([le[0], le[1]]).into(), 16),
            Encoding::I24 => {
                let sign = if le[2] & 0x80 == 0 { 0 } else { 0xff };
                from_int(i32::from_le_bytes([le[0], le[1], le[2], sign]), 24)
            }
            Encoding::I32 => from_int(word(le), 32),
            Encoding::I32Low { bits } => from_int(word(le), bits),
            Encoding::F32 => f32::from_le_bytes([le[0], le[1], le[2], le[3]]),
            Encoding::F64 => {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(le);
                f64::from_le_bytes(bytes) as f32
            }
        }
    }

    /// Copies little-endian `le` into `out` in this format's byte order.
    fn put(self, le: &[u8], out: &mut [u8]) {
        out.copy_from_slice(le);
        if self.big_endian {
            out.reverse();
        }
    }
}

/// The largest positive `bits`-bit signed integer, which full scale maps to.
/// Both directions use it, so ±1.0 survive a round trip exactly.
fn full_scale(bits: u32) -> f64 {
    ((1i64 << (bits - 1)) - 1) as f64
}

/// Scales a sample to a `bits`-bit signed integer, clipping at full scale.
fn to_int(sample: f32, bits: u32) -> i32 {
    (f64::from(sample).clamp(-1.0, 1.0) * full_scale(bits)).round() as i32
}

/// Scales a `bits`-bit signed integer to a sample.
fn from_int(value: i32, bits: u32) -> f32 {
    (f64::from(value) / full_scale(bits)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENCODINGS: [Encoding; 9] = [
        Encoding::I16,
        Encoding::I24,
        Encoding::I32,
        Encoding::F32,
        Encoding::F64,
        Encoding::I32Low { bits: 16 },
        Encoding::I32Low { bits: 18 },
        Encoding::I32Low { bits: 20 },
        Encoding::I32Low { bits: 24 },
    ];

    fn resolution(encoding: Encoding) -> f32 {
        match encoding {
            Encoding::I16 | Encoding::I32Low { bits: 16 } => 1.0 / 32_768.0,
            Encoding::I32Low { bits: 18 } => 1.0 / 131_072.0,
            Encoding::I32Low { bits: 20 } => 1.0 / 524_288.0,
            Encoding::I24 | Encoding::I32Low { .. } => 1.0 / 8_388_608.0,
            Encoding::I32 => 1.0 / 2_147_483_648.0,
            Encoding::F32 | Encoding::F64 => 0.0,
        }
    }

    #[test]
    fn every_format_round_trips_within_one_step() {
        let source = [0.0, 0.5, -0.5, 0.999, -1.0, 0.123_456, -0.000_1];
        for encoding in ENCODINGS {
            for big_endian in [false, true] {
                let format = WireFormat {
                    encoding,
                    big_endian,
                };
                let mut bytes = vec![0u8; source.len() * format.bytes_per_sample()];
                format.encode(&source, &mut bytes);
                let mut back = [0.0f32; 7];
                format.decode(&bytes, &mut back);
                for (a, b) in source.iter().zip(back) {
                    assert!(
                        (a - b).abs() <= resolution(encoding),
                        "{format:?}: {a} came back as {b}"
                    );
                }
            }
        }
    }

    #[test]
    fn integers_land_on_known_values_in_both_byte_orders() {
        let le = WireFormat {
            encoding: Encoding::I16,
            big_endian: false,
        };
        let be = WireFormat {
            big_endian: true,
            ..le
        };
        let mut bytes = [0u8; 2];
        le.encode(&[0.5], &mut bytes);
        assert_eq!(bytes, 16_384i16.to_le_bytes());
        be.encode(&[0.5], &mut bytes);
        assert_eq!(bytes, 16_384i16.to_be_bytes());

        let i24 = WireFormat {
            encoding: Encoding::I24,
            big_endian: false,
        };
        let mut bytes = [0u8; 3];
        i24.encode(&[-1.0], &mut bytes);
        assert_eq!(bytes, [0x01, 0x00, 0x80], "-8,388,607 in 24 bits");

        let low = WireFormat {
            encoding: Encoding::I32Low { bits: 24 },
            big_endian: false,
        };
        let mut bytes = [0u8; 4];
        low.encode(&[1.0], &mut bytes);
        assert_eq!(i32::from_le_bytes(bytes), 8_388_607);
    }

    #[test]
    fn samples_beyond_full_scale_clip() {
        let format = WireFormat {
            encoding: Encoding::I16,
            big_endian: false,
        };
        let mut bytes = [0u8; 4];
        format.encode(&[2.0, -3.0], &mut bytes);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), -i16::MAX);
    }
}
