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

/// The SPS and PPS of an H.264 stream an encoder made, kept aside to make the
/// movie's `avcC` box.
#[cfg(any(unix, windows))]
pub(crate) type ParameterSets = Option<(Vec<u8>, Vec<u8>)>;

/// The NAL units of a start-code stream, without their start codes.
#[cfg(any(unix, windows))]
pub(crate) fn nal_units(stream: &[u8]) -> Vec<&[u8]> {
    // Where each unit's bytes begin, after a `00 00 01`.
    let mut starts = Vec::new();
    let mut at = 0;
    while at + 3 <= stream.len() {
        if stream[at..at + 3] == [0, 0, 1] {
            starts.push(at + 3);
            at += 3;
        } else {
            at += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, &from)| {
            let mut to = starts.get(i + 1).map_or(stream.len(), |next| next - 3);
            // A four-byte start code leaves a zero on the unit before it,
            // and trailing zeros are never part of a unit.
            while to > from && stream[to - 1] == 0 {
                to -= 1;
            }
            &stream[from..to]
        })
        .collect()
}

/// An H.264 packet of start-code NAL units as a movie stores it: each unit
/// behind its four-byte length, the parameter sets left out and kept in
/// `sets` (the first of each). Whether the packet starts a group (holds an
/// IDR picture) comes with it.
#[cfg(any(unix, windows))]
pub(crate) fn length_prefixed(packet: &[u8], sets: &mut ParameterSets) -> (Vec<u8>, bool) {
    let mut data = Vec::with_capacity(packet.len());
    let mut sync = false;
    for nal in nal_units(packet) {
        match nal.first().map(|b| b & 0x1f) {
            Some(7) => {
                if sets.is_none() {
                    *sets = Some((nal.to_vec(), Vec::new()));
                }
            }
            Some(8) => {
                if let Some((_, pps)) = sets
                    && pps.is_empty()
                {
                    *pps = nal.to_vec();
                }
            }
            kind => {
                sync |= kind == Some(5);
                data.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                data.extend_from_slice(nal);
            }
        }
    }
    (data, sync)
}

/// The `avcC` box for the parameter sets of an H.264 stream, or `None`
/// before both have been seen.
#[cfg(any(unix, windows))]
pub(crate) fn avcc(sets: &ParameterSets) -> Option<Vec<u8>> {
    let (sps, pps) = sets
        .as_ref()
        .filter(|(sps, pps)| sps.len() >= 4 && !pps.is_empty())?;
    let mut avcc = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
    avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(sps);
    avcc.push(1);
    avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(pps);
    // The high profiles carry the picture format after the sets: 4:2:0,
    // eight bits.
    if matches!(sps[1], 100 | 110 | 122 | 144) {
        avcc.extend_from_slice(&[0xfd, 0xf8, 0xf8, 0]);
    }
    Some(avcc)
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
