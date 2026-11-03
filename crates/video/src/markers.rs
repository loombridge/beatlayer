//! The markers a movie carries (R12): the samples of the text track its
//! video or audio track names as its chapters, each a title shown from its
//! time.

use std::io::{Read, Seek, SeekFrom};

use crate::boxes::{Be, boxes, child, malformed};
use crate::edits::EditList;
use crate::error::VideoError;
use crate::movie::{
    Opener, chunk_offsets, decode_times, media_header, sample_offsets, sample_sizes,
};
use crate::rational::Rational;

/// The most markers read from a movie.
const MAX_MARKERS: usize = 4096;
/// The longest marker title read, in bytes.
const MAX_TITLE: u32 = 1024;

/// A marker embedded in a movie: a name at a time on the movie's timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    /// Where the marker is, through the chapter track's edit list.
    pub time: Rational,
    /// The name the movie gives it.
    pub name: String,
}

/// The ID of the track a video or audio track names as its chapters.
pub(crate) fn chapter_track(trak: &[u8]) -> Option<u32> {
    let ids = child(child(trak, b"tref")?, b"chap")?;
    Some(u32::from_be_bytes(ids.get(..4)?.try_into().ok()?))
}

fn track_id(trak: &[u8]) -> Option<u32> {
    let tkhd = child(trak, b"tkhd")?;
    let at = if *tkhd.first()? == 1 { 20 } else { 12 };
    Some(u32::from_be_bytes(tkhd.get(at..at + 4)?.try_into().ok()?))
}

/// The markers of the text track `id`, by time.
pub(crate) fn read(
    moov: &[u8],
    id: u32,
    movie_scale: u32,
    opener: &Opener,
) -> Result<Vec<Marker>, VideoError> {
    let trak = boxes(moov)
        .find(|(kind, trak)| kind == b"trak" && track_id(trak) == Some(id))
        .map(|(_, trak)| trak)
        .ok_or_else(|| malformed("a chapter track that isn't there"))?;
    let mdia = child(trak, b"mdia").ok_or_else(|| malformed("no mdia box"))?;
    let handler = child(mdia, b"hdlr")
        .and_then(|h| h.get(8..12))
        .unwrap_or_default();
    if !matches!(handler, b"text" | b"sbtl") {
        return Err(malformed("a chapter track that isn't text"));
    }
    let (media_scale, media_duration) =
        media_header(child(mdia, b"mdhd").ok_or_else(|| malformed("no mdhd box"))?)?;
    let stbl = child(
        child(mdia, b"minf").ok_or_else(|| malformed("no minf box"))?,
        b"stbl",
    )
    .ok_or_else(|| malformed("no stbl box"))?;
    let required = |kind: &[u8; 4]| child(stbl, kind).ok_or_else(|| malformed("a missing table"));
    let sizes = sample_sizes(required(b"stsz")?)?;
    let durations = decode_times(required(b"stts")?, sizes.len())?;
    let offsets = sample_offsets(
        required(b"stsc")?,
        &chunk_offsets(child(stbl, b"stco"), child(stbl, b"co64"))?,
        &sizes,
    )?;
    let elst = child(trak, b"edts").and_then(|e| child(e, b"elst"));
    let edits = EditList::parse(elst, movie_scale, media_scale, media_duration)?;

    let mut reader = opener()?;
    let mut markers = Vec::new();
    let mut at = 0i64;
    for ((&offset, &size), &duration) in offsets.iter().zip(&sizes).zip(&durations) {
        if markers.len() == MAX_MARKERS {
            break;
        }
        let pts = at;
        at = at.saturating_add(i64::from(duration));
        let Some(time) = edits.to_timeline(pts) else {
            continue;
        };
        let mut sample = vec![0; size.min(MAX_TITLE + 2) as usize];
        reader.seek(SeekFrom::Start(offset))?;
        reader.read_exact(&mut sample)?;
        if let Some(name) = title(&sample) {
            markers.push(Marker { time, name });
        }
    }
    markers.sort_by_key(|marker| marker.time);
    Ok(markers)
}

/// A text sample's title: a 16-bit length, then the text in UTF-8 or, after
/// a byte order mark, UTF-16. `None` for an empty one.
fn title(sample: &[u8]) -> Option<String> {
    let mut be = Be::new(sample);
    let length = usize::from(be.u16().ok()?);
    let text = be.rest().get(..length.min(be.remaining()))?;
    let name = match text {
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        _ => String::from_utf8_lossy(text).into_owned(),
    };
    let name = name.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| unit(*pair))
        .collect();
    String::from_utf16_lossy(&units)
}
