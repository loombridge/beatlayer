//! H.264 and HEVC as the movie stores them (a length in front of each NAL
//! unit, the parameter sets in a configuration box) and as FFmpeg and Media
//! Foundation take them (start codes, the parameter sets in the stream).

use crate::decoder::Codec;

/// A codec's parameter sets and the width of the length in front of each NAL
/// unit, read from its `avcC` or `hvcC` box.
pub(crate) struct Config {
    /// SPS and PPS for H.264; VPS, SPS and PPS for HEVC; in stream order.
    pub parameter_sets: Vec<Vec<u8>>,
    pub length_size: usize,
}

impl Config {
    pub fn parse(codec: Codec, extra: &[u8]) -> Option<Self> {
        match codec {
            Codec::H264 => Self::avcc(extra),
            Codec::Hevc => Self::hvcc(extra),
            _ => None,
        }
    }

    fn avcc(extra: &[u8]) -> Option<Self> {
        // Version, profile, compatibility, level, then six reserved bits and
        // the length size less one.
        let length_size = usize::from(extra.get(4)? & 3) + 1;
        let mut at = 5;
        let mut sets = Vec::new();
        // The SPS count is in the low five bits; the PPS count is a byte.
        let count = usize::from(extra.get(at)? & 0x1f);
        at += 1;
        read_sets(extra, &mut at, count, &mut sets)?;
        let count = usize::from(*extra.get(at)?);
        at += 1;
        read_sets(extra, &mut at, count, &mut sets)?;
        (!sets.is_empty()).then_some(Self {
            parameter_sets: sets,
            length_size,
        })
    }

    fn hvcc(extra: &[u8]) -> Option<Self> {
        let length_size = usize::from(extra.get(21)? & 3) + 1;
        let arrays = usize::from(*extra.get(22)?);
        let mut at = 23;
        let mut sets = Vec::new();
        for _ in 0..arrays {
            let count = usize::from(u16::from_be_bytes([
                *extra.get(at + 1)?,
                *extra.get(at + 2)?,
            ]));
            at += 3;
            read_sets(extra, &mut at, count, &mut sets)?;
        }
        (!sets.is_empty()).then_some(Self {
            parameter_sets: sets,
            length_size,
        })
    }

    /// A sample in start-code form, behind the parameter sets if `with_sets`.
    /// NAL units the sample's lengths cut off are left out.
    pub fn to_annex_b(&self, sample: &[u8], with_sets: bool) -> Vec<u8> {
        const START: [u8; 4] = [0, 0, 0, 1];
        let mut out = Vec::with_capacity(sample.len() + 64);
        if with_sets {
            for set in &self.parameter_sets {
                out.extend_from_slice(&START);
                out.extend_from_slice(set);
            }
        }
        let mut at = 0;
        while let Some(len_bytes) = sample.get(at..at + self.length_size) {
            let len = len_bytes
                .iter()
                .fold(0usize, |n, &b| n << 8 | usize::from(b));
            at += self.length_size;
            let Some(nal) = sample.get(at..at.saturating_add(len)) else {
                break;
            };
            out.extend_from_slice(&START);
            out.extend_from_slice(nal);
            at += len;
        }
        out
    }
}

/// Reads `count` parameter sets, each behind a two-byte length.
fn read_sets(extra: &[u8], at: &mut usize, count: usize, sets: &mut Vec<Vec<u8>>) -> Option<()> {
    for _ in 0..count {
        let len = usize::from(u16::from_be_bytes([*extra.get(*at)?, *extra.get(*at + 1)?]));
        sets.push(extra.get(*at + 2..*at + 2 + len)?.to_vec());
        *at += 2 + len;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_avcc_and_converts_a_sample() {
        // Version 1, profile, compat, level, 4-byte lengths, one SPS (2
        // bytes), one PPS (1 byte).
        let avcc = [1, 66, 0, 30, 0xff, 0xe1, 0, 2, 0x67, 0x42, 1, 0, 1, 0x68];
        let config = Config::parse(Codec::H264, &avcc).expect("a config");
        assert_eq!(config.length_size, 4);
        assert_eq!(config.parameter_sets, vec![vec![0x67, 0x42], vec![0x68]]);
        let sample = [0, 0, 0, 3, 0x65, 1, 2, 0, 0, 0, 9, 7];
        let annex_b = config.to_annex_b(&sample, true);
        assert_eq!(
            annex_b,
            [
                0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0, 0, 0, 1, 0x65, 1, 2
            ]
        );
    }
}
