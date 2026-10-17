//! The sampler metadata a WAV file may carry (WP-5.13; the multisampler
//! spec, R3): the root key and the first loop of its `smpl` chunk.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// What a file's `smpl` chunk says about how to play it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SamplerInfo {
    /// The MIDI key that plays the sample at its own pitch, 0 to 127.
    pub root_key: Option<u8>,
    /// The first loop: its first frame, and the frame after its last.
    pub loop_points: Option<(u64, u64)>,
}

/// Reads the `smpl` chunk of the WAV, RF64 or BW64 file at `path`. A file
/// that isn't one of those, has no such chunk, or can't be read has no
/// metadata.
#[must_use]
pub fn read_sampler_info(path: &Path) -> SamplerInfo {
    read(path).unwrap_or_default()
}

fn read(path: &Path) -> Option<SamplerInfo> {
    let mut file = File::open(path).ok()?;
    let mut head = [0u8; 12];
    file.read_exact(&mut head).ok()?;
    if !matches!(&head[..4], b"RIFF" | b"RF64" | b"BW64") || &head[8..12] != b"WAVE" {
        return None;
    }
    loop {
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk).ok()?;
        let size = u64::from(u32::from_le_bytes(chunk[4..8].try_into().ok()?));
        if &chunk[..4] == b"smpl" {
            let mut body = vec![0u8; usize::try_from(size.min(1 << 20)).ok()?];
            file.read_exact(&mut body).ok()?;
            return parse(&body);
        }
        // Chunks are padded to an even length. A size of 0xFFFFFFFF in an
        // RF64 file says the real size is elsewhere, which only the data
        // chunk has, and the smpl chunk comes before or after it.
        if size == u64::from(u32::MAX) {
            return None;
        }
        file.seek(SeekFrom::Current(i64::try_from(size + (size & 1)).ok()?))
            .ok()?;
    }
}

fn parse(body: &[u8]) -> Option<SamplerInfo> {
    let word = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(body.get(at..at + 4)?.try_into().ok()?))
    };
    let key = word(12)?;
    let loops = word(28)?;
    // Each loop: id, type, start, end (the last frame, inclusive), fraction,
    // play count; they follow the 36-byte header and its sampler data.
    let extra = word(32)? as usize;
    let first = 36 + extra;
    let loop_points = (loops > 0)
        .then(|| {
            Some((
                u64::from(word(first + 8)?),
                u64::from(word(first + 12)?) + 1,
            ))
        })
        .flatten()
        .filter(|(start, end)| start < end);
    Some(SamplerInfo {
        root_key: u8::try_from(key).ok().filter(|k| *k <= 127),
        loop_points,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_with_smpl(key: u32, loops: &[(u32, u32)]) -> Vec<u8> {
        let mut smpl = Vec::new();
        for value in [0, 0, 22_675, key, 0, 0, 0, loops.len() as u32, 0] {
            smpl.extend_from_slice(&u32::to_le_bytes(value));
        }
        for (i, (start, end)) in loops.iter().enumerate() {
            for value in [i as u32, 0, *start, *end, 0, 0] {
                smpl.extend_from_slice(&u32::to_le_bytes(value));
            }
        }
        let fmt = [
            1u16.to_le_bytes().as_slice(),
            1u16.to_le_bytes().as_slice(),
            44_100u32.to_le_bytes().as_slice(),
            88_200u32.to_le_bytes().as_slice(),
            2u16.to_le_bytes().as_slice(),
            16u16.to_le_bytes().as_slice(),
        ]
        .concat();
        let mut body = b"WAVE".to_vec();
        for (id, data) in [(b"fmt ", fmt), (b"smpl", smpl), (b"data", vec![0u8; 8])] {
            body.extend_from_slice(id);
            body.extend_from_slice(&u32::to_le_bytes(data.len() as u32));
            body.extend_from_slice(&data);
        }
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&u32::to_le_bytes(body.len() as u32));
        file.extend_from_slice(&body);
        file
    }

    // Spec: multisampler R3
    #[test]
    fn the_sampler_chunk_gives_the_root_key_and_the_first_loop() {
        let dir = std::env::temp_dir().join(format!("bl-smpl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.wav");
        std::fs::write(&path, wav_with_smpl(64, &[(100, 499), (7, 9)])).unwrap();
        assert_eq!(
            read_sampler_info(&path),
            SamplerInfo {
                root_key: Some(64),
                loop_points: Some((100, 500)),
            }
        );
        std::fs::write(&path, wav_with_smpl(60, &[])).unwrap();
        assert_eq!(read_sampler_info(&path).loop_points, None);
        std::fs::write(&path, b"not audio at all").unwrap();
        assert_eq!(read_sampler_info(&path), SamplerInfo::default());
        assert_eq!(
            read_sampler_info(&dir.join("missing.wav")),
            SamplerInfo::default()
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
