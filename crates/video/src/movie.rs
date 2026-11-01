//! The QuickTime demuxer (R1, R2): a movie's tracks, sample tables and edit
//! lists, read from its `moov` box.

use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use crate::boxes::{Be, MOOV_LIMIT, boxes, child, malformed};
use crate::edits::EditList;
use crate::error::VideoError;
use crate::rational::Rational;

/// Anything a movie can be read from.
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// The most samples a track may declare: 19 hours of 60 frames a second. Each
/// costs some 60 bytes while the tables are read.
const MAX_SAMPLES: usize = 1 << 22;
/// The widest or tallest picture read.
pub const MAX_DIMENSION: u32 = 16_384;
/// The most pixels a picture may have: 8,192 by 8,192.
pub const MAX_PIXELS: u64 = 1 << 26;
/// The largest coded frame read into memory.
const MAX_SAMPLE_BYTES: u32 = 256 << 20;

type Opener = Arc<dyn Fn() -> io::Result<Box<dyn ReadSeek>> + Send + Sync>;

/// One coded frame in the movie's file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Where its bytes start in the file.
    pub offset: u64,
    /// How many bytes it has.
    pub size: u32,
    /// Its presentation time, in the track's media time scale units.
    pub pts: i64,
    /// How long it shows, in the track's media time scale units.
    pub duration: u32,
    /// Whether decoding can start here.
    pub sync: bool,
}

/// A movie's picture: its first video track (R1).
#[derive(Clone, Debug)]
pub struct VideoTrack {
    codec: [u8; 4],
    width: u32,
    height: u32,
    extra: Vec<u8>,
    edits: EditList,
    /// In decode order.
    samples: Vec<Sample>,
    /// Indices into `samples`, by presentation time.
    presentation: Vec<u32>,
}

/// A movie's sound: its first audio track (R1). The samples themselves are
/// read through `bl-media` (R3), so this keeps what places them.
#[derive(Clone, Debug)]
pub struct AudioTrack {
    edits: EditList,
}

/// A QuickTime movie read from its `moov` box: its times, edits and sample
/// tables, with nothing decoded.
#[derive(Clone)]
pub struct Movie {
    opener: Opener,
    time_scale: u32,
    duration: u64,
    video: Option<VideoTrack>,
    audio: Option<AudioTrack>,
}

impl std::fmt::Debug for Movie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Movie")
            .field("time_scale", &self.time_scale)
            .field("duration", &self.duration)
            .field("video", &self.video)
            .field("audio", &self.audio)
            .finish_non_exhaustive()
    }
}

impl Movie {
    /// Reads the movie at `path`, and refuses it if no decoder on this
    /// computer takes its video codec (R1).
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read, isn't a QuickTime movie, is damaged,
    /// or has video no decoder takes.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, VideoError> {
        let path = path.as_ref().to_owned();
        let movie = Self::parse(Arc::new(move || {
            Ok(Box::new(File::open(&path)?) as Box<dyn ReadSeek>)
        }))?;
        if let Some(video) = &movie.video {
            crate::decoder::check_decodable(video)?;
        }
        Ok(movie)
    }

    /// Reads a movie held in memory. Unlike [`Movie::open`] it doesn't ask a
    /// decoder to take the video: the fuzz target and the demuxer's tests read
    /// movies whose codecs nothing here decodes.
    ///
    /// # Errors
    ///
    /// Fails if the bytes aren't a QuickTime movie or are damaged.
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Result<Self, VideoError> {
        let bytes: Arc<[u8]> = bytes.into();
        Self::parse(Arc::new(move || {
            Ok(Box::new(Cursor::new(Arc::clone(&bytes))) as Box<dyn ReadSeek>)
        }))
    }

    fn parse(opener: Opener) -> Result<Self, VideoError> {
        let moov = read_moov(&mut *opener()?)?;
        let mvhd = child(&moov, b"mvhd").ok_or_else(|| malformed("no mvhd box"))?;
        let (time_scale, duration) = media_header(mvhd)?;
        if child(&moov, b"mvex").is_some() {
            return Err(VideoError::Unsupported("a fragmented movie".into()));
        }

        let mut video = None;
        let mut audio = None;
        for (kind, trak) in boxes(&moov) {
            if &kind != b"trak" {
                continue;
            }
            let Some(mdia) = child(trak, b"mdia") else {
                continue;
            };
            let handler = child(mdia, b"hdlr")
                .and_then(|h| h.get(8..12))
                .unwrap_or_default();
            match handler {
                b"vide" if video.is_none() => video = Some(video_track(trak, mdia, time_scale)?),
                b"soun" if audio.is_none() => {
                    let (media_scale, media_duration) = media_header(
                        child(mdia, b"mdhd").ok_or_else(|| malformed("no mdhd box"))?,
                    )?;
                    let elst = child(trak, b"edts").and_then(|e| child(e, b"elst"));
                    audio = Some(AudioTrack {
                        edits: EditList::parse(elst, time_scale, media_scale, media_duration)?,
                    });
                }
                _ => {}
            }
        }
        Ok(Self {
            opener,
            time_scale,
            duration,
            video,
            audio,
        })
    }

    /// The movie's length.
    #[must_use]
    pub fn duration(&self) -> Rational {
        Rational::new(i128::from(self.duration), i128::from(self.time_scale))
    }

    /// The picture, if the movie has one.
    #[must_use]
    pub fn video(&self) -> Option<&VideoTrack> {
        self.video.as_ref()
    }

    /// The sound, if the movie has one.
    #[must_use]
    pub fn audio(&self) -> Option<&AudioTrack> {
        self.audio.as_ref()
    }

    /// The index of the frame shown at time `t`, through the edit list: the
    /// last frame whose presentation time is at or before `t` (R2). `None`
    /// where the movie has no video: black (R4).
    #[must_use]
    pub fn frame_at(&self, t: Rational) -> Option<usize> {
        let video = self.video.as_ref()?;
        let media = video.edits.to_media(t)?;
        video.frame_at_media(media)
    }

    /// Opens a fresh reader on the movie's file.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be opened again.
    pub fn reader(&self) -> io::Result<Box<dyn ReadSeek>> {
        (self.opener)()
    }
}

impl VideoTrack {
    /// The codec's four-character code, as the movie names it.
    #[must_use]
    pub fn codec(&self) -> [u8; 4] {
        self.codec
    }

    /// The codec's code as text, for messages.
    #[must_use]
    pub fn codec_name(&self) -> String {
        self.codec
            .iter()
            .map(|&b| {
                if b.is_ascii_graphic() || b == b' ' {
                    b as char
                } else {
                    '?'
                }
            })
            .collect()
    }

    /// The picture's width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The picture's height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The codec's configuration box (`avcC` or `hvcC`), empty for codecs
    /// without one.
    #[must_use]
    pub fn extra(&self) -> &[u8] {
        &self.extra
    }

    /// The track's edit list.
    #[must_use]
    pub fn edits(&self) -> &EditList {
        &self.edits
    }

    /// The number of frames.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.samples.len()
    }

    /// Frame `index`, in decode order.
    #[must_use]
    pub fn sample(&self, index: usize) -> Option<&Sample> {
        self.samples.get(index)
    }

    /// The frame indices by presentation time.
    #[must_use]
    pub fn presentation_order(&self) -> &[u32] {
        &self.presentation
    }

    /// The timeline time at which frame `index` starts to show, through the
    /// edit list, or `None` if no edit plays it (R2).
    #[must_use]
    pub fn frame_start(&self, index: usize) -> Option<Rational> {
        self.edits.to_timeline(self.samples.get(index)?.pts)
    }

    /// The frame shown at media time `media`: the last frame at or before it,
    /// or `None` before the first frame and after the last one's end.
    #[must_use]
    pub fn frame_at_media(&self, media: Rational) -> Option<usize> {
        let units = i64::try_from(media.floor_units(self.edits.media_scale())).ok()?;
        let after = self
            .presentation
            .partition_point(|&i| self.samples[i as usize].pts <= units);
        let last = *self.presentation.get(after.checked_sub(1)?)?;
        let sample = &self.samples[last as usize];
        let shown_until = (after == self.presentation.len())
            .then(|| sample.pts.saturating_add(i64::from(sample.duration)));
        shown_until
            .is_none_or(|end| units < end)
            .then_some(last as usize)
    }

    /// The index of the sync frame decoding for frame `index` starts at: the
    /// last one at or before it in decode order that shows no later than the
    /// frame does. A frame that shows before its sync frame (a leading
    /// picture of an open group of pictures) can lean on the group before.
    #[must_use]
    pub fn sync_before(&self, index: usize) -> usize {
        let Some(frame) = self.samples.get(index) else {
            return 0;
        };
        self.samples[..=index]
            .iter()
            .rposition(|s| s.sync && s.pts <= frame.pts)
            .unwrap_or(0)
    }

    /// Reads frame `index`'s coded bytes.
    ///
    /// # Errors
    ///
    /// Fails if the frame is past the end of the file, or declares more than
    /// a frame may hold.
    pub fn read_sample(&self, reader: &mut dyn ReadSeek, index: usize) -> io::Result<Vec<u8>> {
        let sample = self
            .samples
            .get(index)
            .ok_or_else(|| io::Error::other("no such frame"))?;
        if sample.size > MAX_SAMPLE_BYTES {
            return Err(io::Error::other("a frame larger than Beatlayer reads"));
        }
        reader.seek(SeekFrom::Start(sample.offset))?;
        // Allocate as the bytes arrive: a damaged size past the end of the
        // file doesn't reserve what isn't there.
        let mut bytes = Vec::new();
        reader
            .take(u64::from(sample.size))
            .read_to_end(&mut bytes)?;
        if bytes.len() != sample.size as usize {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(bytes)
    }
}

impl AudioTrack {
    /// Where the track's media plays on the movie's timeline: its gaps are
    /// silence (R4).
    #[must_use]
    pub fn edits(&self) -> &EditList {
        &self.edits
    }
}

/// Finds the `moov` box and returns its body, refusing a file that isn't a
/// QuickTime movie or is fragmented.
fn read_moov(reader: &mut dyn ReadSeek) -> Result<Vec<u8>, VideoError> {
    let end = reader.seek(SeekFrom::End(0))?;
    let mut position = 0u64;
    let mut moov = None;
    while end.saturating_sub(position) >= 8 {
        reader.seek(SeekFrom::Start(position))?;
        let mut header = [0; 16];
        reader.read_exact(&mut header[..8])?;
        let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let kind = [header[4], header[5], header[6], header[7]];
        let (size, header_len) = match size32 {
            0 => (end - position, 8),
            1 => {
                reader.read_exact(&mut header[8..16])?;
                (
                    u64::from_be_bytes(header[8..16].try_into().unwrap_or_default()),
                    16,
                )
            }
            n => (u64::from(n), 8),
        };
        if size < header_len || size > end - position {
            return Err(malformed("a box larger than the file"));
        }
        match &kind {
            b"ftyp" => {
                let mut body = vec![0; (size - header_len).min(1024) as usize];
                reader.read_exact(&mut body)?;
                // The major brand, then the minor version, then compatible
                // brands.
                let quicktime = body.get(..4) == Some(b"qt  ")
                    || body
                        .get(8..)
                        .is_some_and(|b| b.as_chunks::<4>().0.iter().any(|c| c == b"qt  "));
                if !quicktime {
                    return Err(VideoError::Unsupported(
                        "not a QuickTime movie (its file type isn't qt)".into(),
                    ));
                }
            }
            b"moov" if moov.is_none() => {
                let body_len = size - header_len;
                if body_len > MOOV_LIMIT {
                    return Err(VideoError::Unsupported("a moov box over 256 MB".into()));
                }
                let mut body = Vec::new();
                reader.take(body_len).read_to_end(&mut body)?;
                if body.len() as u64 != body_len {
                    return Err(malformed("a truncated moov box"));
                }
                moov = Some(body);
            }
            b"moof" => return Err(VideoError::Unsupported("a fragmented movie".into())),
            _ => {}
        }
        position += size;
    }
    moov.ok_or_else(|| VideoError::Unsupported("not a QuickTime movie (it has no moov box)".into()))
}

/// The time scale and duration of an `mvhd` or `mdhd` body.
fn media_header(body: &[u8]) -> Result<(u32, u64), VideoError> {
    let mut be = Be::new(body);
    let version = be.u8()?;
    be.skip(3)?;
    let (scale, duration) = if version == 1 {
        be.skip(16)?;
        (be.u32()?, be.u64()?)
    } else {
        be.skip(8)?;
        let scale = be.u32()?;
        let duration = be.u32()?;
        // 0xFFFF_FFFF means unknown; the sample tables give the length.
        (scale, u64::from(duration))
    };
    if scale == 0 {
        return Err(malformed("a time scale of zero"));
    }
    Ok((scale, duration))
}

fn video_track(trak: &[u8], mdia: &[u8], movie_scale: u32) -> Result<VideoTrack, VideoError> {
    let (media_scale, media_duration) =
        media_header(child(mdia, b"mdhd").ok_or_else(|| malformed("no mdhd box"))?)?;
    let stbl = child(
        child(mdia, b"minf").ok_or_else(|| malformed("no minf box"))?,
        b"stbl",
    )
    .ok_or_else(|| malformed("no stbl box"))?;
    let table = |kind: &[u8; 4]| child(stbl, kind);
    let required = |kind: &[u8; 4]| table(kind).ok_or_else(|| malformed("a missing sample table"));

    let (codec, width, height, extra) = sample_entry(required(b"stsd")?)?;
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(VideoError::Unsupported(format!(
            "a picture of {width} by {height} pixels"
        )));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(VideoError::Unsupported(format!(
            "a picture of {width} by {height} pixels, over the {MAX_PIXELS} Beatlayer reads"
        )));
    }

    let sizes = sample_sizes(required(b"stsz")?)?;
    let count = sizes.len();
    let durations = decode_times(required(b"stts")?, count)?;
    let offsets = sample_offsets(
        required(b"stsc")?,
        &chunk_offsets(table(b"stco"), table(b"co64"))?,
        &sizes,
    )?;
    let composition = composition_offsets(table(b"ctts"), count)?;
    let sync = sync_samples(table(b"stss"), count)?;

    let mut samples = Vec::with_capacity(count);
    let mut dts = 0i64;
    for i in 0..count {
        samples.push(Sample {
            offset: offsets[i],
            size: sizes[i],
            pts: dts.saturating_add(i64::from(composition[i])),
            duration: durations[i],
            sync: sync.as_ref().is_none_or(|s| s.binary_search(&i).is_ok()),
        });
        dts = dts.saturating_add(i64::from(durations[i]));
    }
    // Stable, so frames that share a time stay in decode order.
    let mut presentation: Vec<u32> = (0..count as u32).collect();
    presentation.sort_by_key(|&i| samples[i as usize].pts);

    let elst = child(trak, b"edts").and_then(|e| child(e, b"elst"));
    // The media's length from its own sample table, which `mdhd` can leave
    // unknown.
    let media_duration = if media_duration == u64::from(u32::MAX) {
        dts.max(0) as u64
    } else {
        media_duration
    };
    Ok(VideoTrack {
        codec,
        width,
        height,
        extra,
        edits: EditList::parse(elst, movie_scale, media_scale, media_duration)?,
        samples,
        presentation,
    })
}

/// The first sample entry of an `stsd` body: its codec, picture size, and
/// the codec's configuration box.
fn sample_entry(stsd: &[u8]) -> Result<([u8; 4], u32, u32, Vec<u8>), VideoError> {
    let mut be = Be::new(stsd);
    be.skip(4)?;
    if be.u32()? == 0 {
        return Err(malformed("a track with no sample description"));
    }
    let size = be.u32()? as usize;
    let codec: [u8; 4] = be.take(4)?.try_into().unwrap_or_default();
    let mut entry = Be::new(be.take(size.saturating_sub(8).min(be.remaining()))?);
    // Reserved and data reference, then version, revision, vendor and
    // quality: 24 bytes before the picture's size.
    entry.skip(24)?;
    let width = u32::from(entry.u16()?);
    let height = u32::from(entry.u16()?);
    // Resolution, data size, frame count, compressor name, depth and colour
    // table: 50 bytes before the entry's own boxes.
    entry.skip(50)?;
    let extra = child(entry.rest(), b"avcC")
        .or_else(|| child(entry.rest(), b"hvcC"))
        .unwrap_or_default()
        .to_vec();
    Ok((codec, width, height, extra))
}

/// The entries of a table box: after the version and flags, a count, then
/// `entry_len` bytes each. Refuses a count its body can't hold.
fn entries(body: &[u8], entry_len: usize) -> Result<std::slice::ChunksExact<'_, u8>, VideoError> {
    let mut be = Be::new(body);
    be.skip(4)?;
    let count = be.u32()? as usize;
    let rest = be.rest();
    if count.checked_mul(entry_len).is_none_or(|n| n > rest.len()) {
        return Err(malformed("a sample table longer than its box"));
    }
    Ok(rest[..count * entry_len].chunks_exact(entry_len))
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes[..4].try_into().unwrap_or_default())
}

fn sample_sizes(stsz: &[u8]) -> Result<Vec<u32>, VideoError> {
    let mut be = Be::new(stsz);
    be.skip(4)?;
    let constant = be.u32()?;
    let count = be.u32()? as usize;
    if count > MAX_SAMPLES {
        return Err(VideoError::Unsupported(format!(
            "{count} frames in a track"
        )));
    }
    if constant != 0 {
        return Ok(vec![constant; count]);
    }
    if count.checked_mul(4).is_none_or(|n| n > be.remaining()) {
        return Err(malformed("an stsz box shorter than its count"));
    }
    Ok(be.rest()[..count * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_be_bytes(*c))
        .collect())
}

/// Each sample's duration from `stts`, `count` of them.
fn decode_times(stts: &[u8], count: usize) -> Result<Vec<u32>, VideoError> {
    let mut durations = Vec::with_capacity(count);
    for entry in entries(stts, 8)? {
        let repeat = be32(entry) as usize;
        let delta = be32(&entry[4..]);
        let take = repeat.min(count - durations.len());
        durations.extend(std::iter::repeat_n(delta, take));
    }
    if durations.len() < count {
        return Err(malformed("fewer frame durations than frames"));
    }
    Ok(durations)
}

/// Each sample's offset from presentation to decode time from `ctts`; zero
/// without the box.
fn composition_offsets(ctts: Option<&[u8]>, count: usize) -> Result<Vec<i32>, VideoError> {
    let mut offsets = Vec::with_capacity(count);
    if let Some(ctts) = ctts {
        for entry in entries(ctts, 8)? {
            let take = (be32(entry) as usize).min(count - offsets.len());
            offsets.extend(std::iter::repeat_n(be32(&entry[4..]) as i32, take));
        }
    }
    offsets.resize(count, 0);
    Ok(offsets)
}

/// The sync samples' indices, ascending, or `None` when every frame is one.
fn sync_samples(stss: Option<&[u8]>, count: usize) -> Result<Option<Vec<usize>>, VideoError> {
    let Some(stss) = stss else {
        return Ok(None);
    };
    let mut sync: Vec<usize> = entries(stss, 4)?
        .filter_map(|e| (be32(e) as usize).checked_sub(1))
        .filter(|&i| i < count)
        .collect();
    sync.sort_unstable();
    sync.dedup();
    Ok(Some(sync))
}

fn chunk_offsets(stco: Option<&[u8]>, co64: Option<&[u8]>) -> Result<Vec<u64>, VideoError> {
    if let Some(co64) = co64 {
        Ok(entries(co64, 8)?
            .map(|e| u64::from_be_bytes(e.try_into().unwrap_or_default()))
            .collect())
    } else if let Some(stco) = stco {
        Ok(entries(stco, 4)?.map(|e| u64::from(be32(e))).collect())
    } else {
        Err(malformed("no chunk offsets"))
    }
}

/// Each sample's file offset, from the chunks' offsets, how many samples
/// each chunk holds (`stsc`), and the samples' sizes.
fn sample_offsets(stsc: &[u8], chunks: &[u64], sizes: &[u32]) -> Result<Vec<u64>, VideoError> {
    let runs: Vec<(u32, u32)> = entries(stsc, 12)?
        .map(|e| (be32(e), be32(&e[4..])))
        .collect();
    if runs.first().is_none_or(|r| r.0 != 1) || runs.windows(2).any(|w| w[0].0 >= w[1].0) {
        return Err(malformed("an stsc box out of order"));
    }
    let mut offsets = Vec::with_capacity(sizes.len());
    let mut run = 0;
    for (chunk, &start) in chunks.iter().enumerate() {
        let number = chunk as u32 + 1;
        while runs.get(run + 1).is_some_and(|next| next.0 <= number) {
            run += 1;
        }
        let mut at = start;
        for _ in 0..runs[run].1 {
            let Some(&size) = sizes.get(offsets.len()) else {
                return Ok(offsets);
            };
            offsets.push(at);
            at = at
                .checked_add(u64::from(size))
                .ok_or_else(|| malformed("a frame past the end of the address space"))?;
        }
    }
    if offsets.len() < sizes.len() {
        return Err(malformed("fewer chunks than frames"));
    }
    Ok(offsets)
}
