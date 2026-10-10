//! Encoder delay and padding in MPEG-4 audio files.
//!
//! An AAC encoder adds priming frames before the audio and padding after it,
//! so a decoder that plays every frame starts late and ends long: Apple's
//! encoder primes 2,112 frames, about 48 ms at 44.1 kHz, which would put an
//! imported loop off the grid. Symphonia 0.6 doesn't trim MPEG-4 files, so
//! this module reads the two places encoders record the trim:
//!
//! - the `iTunSMPB` tag (Apple's encoders): priming, padding, and the exact
//!   number of valid frames;
//! - the track's edit list, `elst` (ffmpeg and most other encoders): the media
//!   time where playback starts, and how long it lasts.
//!
//! The tag wins where both exist, because it counts frames exactly.

use std::io::{Read, Seek, SeekFrom};

/// The frames to trim from a decoded MPEG-4 audio track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Trim {
    /// Frames to drop from the start.
    pub priming: u64,
    /// The number of frames to keep after the priming, if recorded.
    pub valid: Option<u64>,
}

/// The largest `moov` box read into memory: far above any audio file's.
const MOOV_LIMIT: u64 = 64 << 20;

/// The trim as the file records it, before the decoded sample rate is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordedTrim {
    /// From `iTunSMPB`, already in frames.
    Frames(Trim),
    /// From an edit list, in timescale units.
    EditList {
        media_time: u64,
        media_scale: u32,
        duration: u64,
        movie_scale: u32,
    },
}

impl RecordedTrim {
    /// The trim in frames at the decoded sample rate.
    pub fn at_rate(self, sample_rate: u32) -> Trim {
        match self {
            Self::Frames(trim) => trim,
            Self::EditList {
                media_time,
                media_scale,
                duration,
                movie_scale,
            } => {
                let rate = u128::from(sample_rate);
                let priming = (u128::from(media_time) * rate / u128::from(media_scale)) as u64;
                // The duration is in movie units, often milliseconds, which
                // needn't land on a frame. Trust it only when it does;
                // otherwise keep the padding rather than cut real audio.
                let scaled = u128::from(duration) * rate;
                let movie_scale = u128::from(movie_scale);
                let valid = (duration > 0 && scaled % movie_scale == 0)
                    .then(|| (scaled / movie_scale) as u64);
                Trim { priming, valid }
            }
        }
    }
}

/// Reads the trim for the first audio track. Returns `None` if the file
/// records none or can't be parsed; the caller then plays every decoded frame.
pub(crate) fn read_trim(reader: &mut (impl Read + Seek)) -> Option<RecordedTrim> {
    let moov = find_moov(reader)?;
    if let Some(trim) = itunsmpb(&moov) {
        return Some(RecordedTrim::Frames(trim));
    }
    edit_list_trim(&moov)
}

/// Finds the top-level `moov` box and returns its body.
fn find_moov(reader: &mut (impl Read + Seek)) -> Option<Vec<u8>> {
    let end = reader.seek(SeekFrom::End(0)).ok()?;
    let mut position = 0u64;
    while position + 8 <= end {
        reader.seek(SeekFrom::Start(position)).ok()?;
        let mut header = [0; 16];
        reader.read_exact(&mut header[..8]).ok()?;
        let size32 = u32::from_be_bytes(header[0..4].try_into().ok()?);
        let kind: [u8; 4] = header[4..8].try_into().ok()?;
        let (size, header_len) = match size32 {
            0 => (end - position, 8),
            1 => {
                reader.read_exact(&mut header[8..16]).ok()?;
                (u64::from_be_bytes(header[8..16].try_into().ok()?), 16)
            }
            n => (u64::from(n), 8),
        };
        if size < header_len {
            return None;
        }
        if &kind == b"moov" {
            let body_len = size - header_len;
            if body_len > MOOV_LIMIT {
                return None;
            }
            let mut body = vec![0; body_len as usize];
            reader.read_exact(&mut body).ok()?;
            return Some(body);
        }
        position = position.checked_add(size)?;
    }
    None
}

/// Iterates the boxes in a body: `(kind, body)`.
fn boxes(mut data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        if data.len() < 8 {
            return None;
        }
        let size32 = u32::from_be_bytes(data[0..4].try_into().ok()?);
        let kind: [u8; 4] = data[4..8].try_into().ok()?;
        let (size, header_len) = match size32 {
            0 => (data.len(), 8),
            1 if data.len() >= 16 => {
                let size = u64::from_be_bytes(data[8..16].try_into().ok()?);
                (usize::try_from(size).ok()?, 16)
            }
            n => (n as usize, 8),
        };
        if size < header_len || size > data.len() {
            return None;
        }
        let body = &data[header_len..size];
        data = &data[size..];
        Some((kind, body))
    })
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).find(|(k, _)| k == kind).map(|(_, body)| body)
}

/// Reads `moov/udta/meta/ilst/----` with the name `iTunSMPB`.
fn itunsmpb(moov: &[u8]) -> Option<Trim> {
    let meta = child(child(moov, b"udta")?, b"meta")?;
    // `meta` is a full box: four bytes of version and flags first, though some
    // writers leave them out. Try both.
    let ilst = child(meta.get(4..)?, b"ilst").or_else(|| child(meta, b"ilst"))?;
    for (kind, item) in boxes(ilst) {
        if &kind != b"----" {
            continue;
        }
        let name = child(item, b"name")?;
        if name.get(4..) != Some(b"iTunSMPB".as_slice()) {
            continue;
        }
        // The value is text after eight bytes of type and locale.
        let data = child(item, b"data")?.get(8..)?;
        let text = std::str::from_utf8(data).ok()?;
        let fields: Vec<u64> = text
            .split_whitespace()
            .map(|f| u64::from_str_radix(f, 16))
            .collect::<Result<_, _>>()
            .ok()?;
        // Fields: reserved, priming, padding, valid frames, then reserved.
        let priming = *fields.get(1)?;
        let valid = fields.get(3).copied().filter(|&v| v > 0);
        return Some(Trim { priming, valid });
    }
    None
}

/// Reads the first audio track's edit list.
fn edit_list_trim(moov: &[u8]) -> Option<RecordedTrim> {
    let movie_scale = timescale(child(moov, b"mvhd")?)?;
    for (kind, trak) in boxes(moov) {
        if &kind != b"trak" {
            continue;
        }
        let mdia = child(trak, b"mdia")?;
        let handler = child(mdia, b"hdlr")?;
        if handler.get(8..12) != Some(b"soun".as_slice()) {
            continue;
        }
        let media_scale = timescale(child(mdia, b"mdhd")?)?;
        let elst = child(child(trak, b"edts")?, b"elst")?;
        let version = *elst.first()?;
        let count = u32::from_be_bytes(elst.get(4..8)?.try_into().ok()?);
        let entry_len = if version == 1 { 20 } else { 12 };
        let mut entries = elst.get(8..)?.chunks_exact(entry_len).take(count as usize);
        // The first entry that isn't an empty edit (media time -1).
        let (duration, media_time) = entries.find_map(|e| {
            let (duration, media_time) = if version == 1 {
                (
                    u64::from_be_bytes(e[0..8].try_into().ok()?),
                    i64::from_be_bytes(e[8..16].try_into().ok()?),
                )
            } else {
                (
                    u64::from(u32::from_be_bytes(e[0..4].try_into().ok()?)),
                    i64::from(i32::from_be_bytes(e[4..8].try_into().ok()?)),
                )
            };
            (media_time >= 0).then_some((duration, media_time as u64))
        })?;
        return Some(RecordedTrim::EditList {
            media_time,
            media_scale,
            duration,
            movie_scale,
        });
    }
    None
}

/// The timescale in an `mvhd` or `mdhd` body.
fn timescale(body: &[u8]) -> Option<u32> {
    let offset = if *body.first()? == 1 { 20 } else { 12 };
    let scale = u32::from_be_bytes(body.get(offset..offset + 4)?.try_into().ok()?);
    (scale > 0).then_some(scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    fn full(version: u8, rest: &[u8]) -> Vec<u8> {
        let mut out = vec![version, 0, 0, 0];
        out.extend_from_slice(rest);
        out
    }

    /// A minimal file: one audio track at 44.1 kHz whose edit list skips
    /// 1,024 priming frames and lasts `millis` milliseconds, and optionally
    /// an `iTunSMPB` tag.
    fn file(itunes: Option<&str>, millis: u32) -> Vec<u8> {
        let mut mvhd = vec![0; 8];
        mvhd.extend_from_slice(&1000u32.to_be_bytes()); // movie timescale
        mvhd.extend_from_slice(&[0; 80]);
        let mut mdhd = vec![0; 8];
        mdhd.extend_from_slice(&44_100u32.to_be_bytes());
        mdhd.extend_from_slice(&[0; 8]);
        let mut hdlr = vec![0; 4];
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0; 13]);
        let mut elst = 1u32.to_be_bytes().to_vec();
        elst.extend_from_slice(&millis.to_be_bytes()); // in movie units
        elst.extend_from_slice(&1024i32.to_be_bytes());
        elst.extend_from_slice(&[0, 1, 0, 0]);

        let mdia = [
            mp4_box(b"mdhd", &full(0, &mdhd)),
            mp4_box(b"hdlr", &full(0, &hdlr)),
        ]
        .concat();
        let trak = [
            mp4_box(b"edts", &mp4_box(b"elst", &full(0, &elst))),
            mp4_box(b"mdia", &mdia),
        ]
        .concat();
        let mut moov = [mp4_box(b"mvhd", &full(0, &mvhd)), mp4_box(b"trak", &trak)].concat();
        if let Some(text) = itunes {
            let mean = mp4_box(b"mean", &[&[0u8; 4][..], b"com.apple.iTunes"].concat());
            let name = mp4_box(b"name", &[&[0u8; 4][..], b"iTunSMPB"].concat());
            let data = mp4_box(
                b"data",
                &[&[0, 0, 0, 1, 0, 0, 0, 0][..], text.as_bytes()].concat(),
            );
            let item = mp4_box(b"----", &[mean, name, data].concat());
            let meta = mp4_box(b"meta", &full(0, &mp4_box(b"ilst", &item)));
            moov.extend(mp4_box(b"udta", &meta));
        }
        [
            mp4_box(b"ftyp", b"M4A \0\0\0\0"),
            mp4_box(b"moov", &moov),
            mp4_box(b"mdat", &[0; 16]),
        ]
        .concat()
    }

    #[test]
    fn reads_the_edit_list() {
        let trim = read_trim(&mut Cursor::new(file(None, 1000))).map(|t| t.at_rate(44_100));
        assert_eq!(
            trim,
            Some(Trim {
                priming: 1024,
                valid: Some(44_100)
            })
        );
    }

    #[test]
    fn prefers_itunsmpb() {
        let tag = " 00000000 00000840 000001C0 000000000000AC44 00000000";
        let trim = read_trim(&mut Cursor::new(file(Some(tag), 1000))).map(|t| t.at_rate(44_100));
        assert_eq!(
            trim,
            Some(Trim {
                priming: 2112,
                valid: Some(44_100)
            })
        );
    }

    #[test]
    fn scales_the_edit_list_to_the_decoded_rate() {
        // An HE-AAC file decodes at twice its media timescale.
        let trim = read_trim(&mut Cursor::new(file(None, 1000)))
            .unwrap()
            .at_rate(88_200);
        assert_eq!(trim.priming, 2048);
        assert_eq!(trim.valid, Some(88_200));
    }

    #[test]
    fn keeps_the_padding_when_the_duration_is_inexact() {
        // 1,001 ms at 44.1 kHz is 44,144.1 frames.
        let trim = read_trim(&mut Cursor::new(file(None, 1001)))
            .unwrap()
            .at_rate(44_100);
        assert_eq!(trim.priming, 1024);
        assert_eq!(trim.valid, None);
    }

    #[test]
    fn ignores_files_without_moov() {
        let mut cursor = Cursor::new(mp4_box(b"ftyp", b"M4A \0\0\0\0"));
        assert_eq!(read_trim(&mut cursor), None);
    }
}
