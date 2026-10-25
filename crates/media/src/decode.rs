//! Decoding audio files to 32-bit float samples.
//!
//! [`Decoder`] reads any supported file a block at a time and can seek, which
//! is what disk streaming (WP-1.3) and peak building (WP-1.2) need.
//! [`decode_file`] reads a whole file into [`SampleData`]. Both run on worker
//! threads, never on an audio thread: they read files and allocate.

use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known as codec_ids;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::{Error as SymphoniaError, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::well_known as format_ids;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::core::units::{TimeBase, Timestamp};

use crate::error::{MediaError, from_symphonia};
use crate::limits;
use crate::mp4::{self, RecordedTrim};
use crate::rf64::{Rf64Reader, is_rf64};
use crate::sample_data::SampleData;

/// The file format around the audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Container {
    /// RIFF WAVE, including Broadcast Wave under 4 GB.
    Wav,
    /// RF64 or BW64: WAVE with 64-bit sizes, used for files over 4 GB.
    Rf64,
    /// AIFF or AIFF-C.
    Aiff,
    /// Native FLAC.
    Flac,
    /// An MPEG audio stream (`.mp3`).
    Mp3,
    /// Ogg.
    Ogg,
    /// ISO MPEG-4 (`.m4a`, `.mp4`).
    Mp4,
    /// A raw AAC stream in ADTS framing (`.aac`).
    Adts,
    /// Any other container symphonia reads.
    Other,
}

/// How the audio is coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Codec {
    /// Uncompressed integer or float PCM, including A-law and mu-law.
    Pcm,
    /// ADPCM.
    Adpcm,
    /// FLAC.
    Flac,
    /// Apple Lossless.
    Alac,
    /// MPEG-1 or MPEG-2 Layer III.
    Mp3,
    /// Vorbis.
    Vorbis,
    /// AAC (low complexity).
    Aac,
    /// Any other codec symphonia decodes.
    Other,
}

impl Codec {
    /// Whether decoding gives back exactly the samples that were encoded.
    #[must_use]
    pub fn is_lossless(self) -> bool {
        matches!(self, Self::Pcm | Self::Flac | Self::Alac)
    }
}

/// What a media file holds, read from its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaInfo {
    /// The file format.
    pub container: Container,
    /// The audio coding.
    pub codec: Codec,
    /// Frames per second.
    pub sample_rate: u32,
    /// The number of channels.
    pub channels: u16,
    /// The number of frames the file plays, if its header says. Decoding
    /// gives the exact count.
    pub frames: Option<u64>,
    /// The bits per sample of the source, for PCM and lossless codecs.
    pub bits_per_sample: Option<u32>,
}

/// Reads frames from a media file, in order, with seeking.
pub struct Decoder {
    info: MediaInfo,
    inner: Inner,
    /// The next frame [`Decoder::read`] returns.
    position: u64,
}

enum Inner {
    Rf64(Rf64Reader<Box<dyn ReadSeek>>),
    Symphonia(Box<SymphoniaStream>),
}

impl fmt::Debug for Decoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decoder")
            .field("info", &self.info)
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

impl Decoder {
    /// Opens a file and reads its header.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read, isn't a format Beatlayer reads, or
    /// has no audio track.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MediaError> {
        let path = path.as_ref();
        let extension = path.extension().and_then(|e| e.to_str());
        Self::from_reader(File::open(path)?, extension)
    }

    /// Reads media from any seekable reader, whose first byte is the start of
    /// the file. The file extension, if known, helps the format probe.
    ///
    /// # Errors
    ///
    /// As for [`Decoder::open`].
    pub fn from_reader<R>(mut reader: R, extension: Option<&str>) -> Result<Self, MediaError>
    where
        R: Read + Seek + Send + 'static,
    {
        let mut magic = [0; 8];
        reader.seek(SeekFrom::Start(0))?;
        let got = read_up_to(&mut reader, &mut magic)?;
        let trim = if got == 8 && &magic[4..8] == b"ftyp" {
            mp4::read_trim(&mut reader)
        } else {
            None
        };
        reader.seek(SeekFrom::Start(0))?;

        if got >= 4 && is_rf64(&magic[0..4]) {
            let rf64 = Rf64Reader::open(Box::new(reader) as Box<dyn ReadSeek>)?;
            let format = *rf64.format();
            let info = MediaInfo {
                container: Container::Rf64,
                codec: Codec::Pcm,
                sample_rate: format.sample_rate,
                channels: format.channels,
                frames: Some(format.frames),
                bits_per_sample: Some(format.bits_per_sample),
            };
            return Ok(Self {
                info,
                inner: Inner::Rf64(rf64),
                position: 0,
            });
        }

        let reader: Box<dyn ReadSeek> = match fixed_block_size_quirk(&mut reader)? {
            Some(block_size) => Box::new(FlacBlockSizeFix::new(reader, block_size)),
            None => Box::new(reader),
        };
        let source = Box::new(SeekableSource::new(reader)?);
        let stream = SymphoniaStream::open(source, extension, trim)?;
        Ok(Self {
            info: stream.info,
            inner: Inner::Symphonia(Box::new(stream)),
            position: 0,
        })
    }

    /// What the file holds.
    #[must_use]
    pub fn info(&self) -> &MediaInfo {
        &self.info
    }

    /// The frame the next [`Decoder::read`] starts at.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.position
    }

    /// The number of packets the codec rejected as damaged so far. Each one
    /// is replaced with silence of its length, so decoding carries on and
    /// what follows stays in time. (Damage the container reader can't frame
    /// is dropped or ends the file, as symphonia's readers decide.)
    #[must_use]
    pub fn damaged_packets(&self) -> u64 {
        match &self.inner {
            Inner::Rf64(_) => 0,
            Inner::Symphonia(s) => s.damaged,
        }
    }

    /// Reads frames into planar outputs, one slice per channel, up to the
    /// shortest slice's length. Returns the number of frames read, which is
    /// zero only at the end of the file.
    ///
    /// # Errors
    ///
    /// Fails if the number of slices isn't the channel count, or on a read
    /// error.
    pub fn read(&mut self, out: &mut [&mut [f32]]) -> Result<usize, MediaError> {
        if out.len() != usize::from(self.info.channels) {
            return Err(MediaError::Malformed(format!(
                "{} output channels for {}-channel audio",
                out.len(),
                self.info.channels
            )));
        }
        let frames = match &mut self.inner {
            Inner::Rf64(r) => r.read(out)?,
            Inner::Symphonia(s) => s.read(out)?,
        };
        self.position += frames as u64;
        Ok(frames)
    }

    /// Moves to a frame, so the next read starts there. Seeking past the end
    /// leaves the decoder at the end.
    ///
    /// Seeks are sample-accurate: the decoder seeks to a point before the
    /// frame and decodes forward to it.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read or seeked.
    pub fn seek(&mut self, frame: u64) -> Result<(), MediaError> {
        match &mut self.inner {
            Inner::Rf64(r) => {
                r.seek(frame)?;
                self.position = frame.min(r.format().frames);
            }
            Inner::Symphonia(s) => {
                s.seek(frame)?;
                self.position = self.info.frames.map_or(frame, |end| frame.min(end));
            }
        }
        Ok(())
    }

    /// Decodes the rest of the file into memory.
    ///
    /// # Errors
    ///
    /// As for [`Decoder::read`].
    pub fn decode_to_end(mut self) -> Result<SampleData, MediaError> {
        let channels = usize::from(self.info.channels);
        // The header's length is a hint: reserve no more than 64 MiB for it
        // over all channels, and let the planes grow as the audio arrives.
        let expected = self
            .info
            .frames
            .map_or(0, |f| f.saturating_sub(self.position))
            .min((1 << 24) / channels.max(1) as u64) as usize;
        let mut planes: Vec<Vec<f32>> = (0..channels)
            .map(|_| Vec::with_capacity(expected))
            .collect();
        let mut block: Vec<Vec<f32>> = vec![vec![0.0; 1 << 14]; channels];
        loop {
            let mut outs: Vec<&mut [f32]> = block.iter_mut().map(|c| &mut c[..]).collect();
            let n = self.read(&mut outs)?;
            if n == 0 {
                break;
            }
            for (plane, src) in planes.iter_mut().zip(&block) {
                plane.extend_from_slice(&src[..n]);
            }
        }
        SampleData::new(self.info.sample_rate, planes)
    }
}

/// Reads a file's header without decoding its audio.
///
/// # Errors
///
/// As for [`Decoder::open`].
pub fn probe(path: impl AsRef<Path>) -> Result<MediaInfo, MediaError> {
    Ok(*Decoder::open(path)?.info())
}

/// Decodes a whole file into memory at its own sample rate.
///
/// # Errors
///
/// As for [`Decoder::open`] and [`Decoder::read`].
pub fn decode_file(path: impl AsRef<Path>) -> Result<SampleData, MediaError> {
    Decoder::open(path)?.decode_to_end()
}

/// A readable, seekable, sendable byte source.
pub(crate) trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// Where a FLAC file's stream header keeps the minimum block size: after
/// the `fLaC` marker and the STREAMINFO block's own four-byte header.
const FLAC_MIN_BLOCK_OFFSET: u64 = 8;

/// The block size of a FLAC stream whose frames all have one fixed block
/// size but whose stream header gives a smaller minimum, or `None` for any
/// other file. Some encoders (flacenc's whole-stream encoder among them)
/// count a fixed-blocksize stream's short last block in the minimum, which
/// the format leaves out; symphonia then takes the stream for a
/// variable-blocksize one and refuses every frame, though other decoders
/// read it. Leaves the reader at its start.
fn fixed_block_size_quirk(reader: &mut (impl Read + Seek)) -> io::Result<Option<u16>> {
    let found = find_fixed_block_size_quirk(reader);
    reader.seek(SeekFrom::Start(0))?;
    found
}

fn find_fixed_block_size_quirk(reader: &mut (impl Read + Seek)) -> io::Result<Option<u16>> {
    // `fLaC`, the STREAMINFO block's header, and its 34 bytes.
    let mut head = [0; 42];
    reader.seek(SeekFrom::Start(0))?;
    if read_up_to(reader, &mut head)? < head.len() || &head[0..4] != b"fLaC" || head[4] & 0x7f != 0
    {
        return Ok(None);
    }
    let min = u16::from_be_bytes([head[8], head[9]]);
    let max = u16::from_be_bytes([head[10], head[11]]);
    if min >= max {
        return Ok(None);
    }
    // Past the metadata blocks to the first frame.
    let mut last = head[4] & 0x80 != 0;
    let mut offset = 42;
    while !last {
        let mut block = [0; 4];
        reader.seek(SeekFrom::Start(offset))?;
        if read_up_to(reader, &mut block)? < block.len() {
            return Ok(None);
        }
        last = block[0] & 0x80 != 0;
        offset += 4 + u64::from(u32::from_be_bytes([0, block[1], block[2], block[3]]));
    }
    let mut frame = [0; 2];
    reader.seek(SeekFrom::Start(offset))?;
    if read_up_to(reader, &mut frame)? < frame.len() {
        return Ok(None);
    }
    // A frame starts with the 14-bit sync code, a reserved 0 bit, and the
    // blocking strategy bit, 0 for a fixed block size: `0xFFF8` in all.
    let fixed = frame == [0xff, 0xf8];
    Ok(fixed.then_some(max))
}

/// Reads a FLAC file with [`fixed_block_size_quirk`] as if its stream
/// header gave the right minimum block size, the stream's fixed one. The
/// file itself isn't changed.
struct FlacBlockSizeFix<R> {
    inner: R,
    position: u64,
    /// The minimum block size's two bytes as they should read.
    min_block: [u8; 2],
}

impl<R> FlacBlockSizeFix<R> {
    fn new(inner: R, block_size: u16) -> Self {
        Self {
            inner,
            position: 0,
            min_block: block_size.to_be_bytes(),
        }
    }
}

impl<R: Read> Read for FlacBlockSizeFix<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        let start = self.position;
        let end = start + n as u64;
        for (byte, at) in self.min_block.iter().zip(FLAC_MIN_BLOCK_OFFSET..) {
            if (start..end).contains(&at)
                && let Ok(index) = usize::try_from(at - start)
            {
                buf[index] = *byte;
            }
        }
        self.position = end;
        Ok(n)
    }
}

impl<R: Seek> Seek for FlacBlockSizeFix<R> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.position = self.inner.seek(to)?;
        Ok(self.position)
    }
}

/// Adapts any `Read + Seek` to symphonia's `MediaSource`.
///
/// Symphonia requires `Sync` of a source, though one decoder reads it from
/// one thread at a time; the mutex supplies `Sync` for readers that are only
/// `Send`, and is never contended.
struct SeekableSource<R> {
    inner: Mutex<R>,
    length: u64,
}

impl<R: Read + Seek> SeekableSource<R> {
    fn new(mut inner: R) -> io::Result<Self> {
        let here = inner.stream_position()?;
        let length = inner.seek(SeekFrom::End(0))?;
        inner.seek(SeekFrom::Start(here))?;
        Ok(Self {
            inner: Mutex::new(inner),
            length,
        })
    }

    fn reader(&mut self) -> &mut R {
        // A reader poisoned by a panic is only a byte position; reading on
        // is safe.
        self.inner.get_mut().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<R: Read + Seek> Read for SeekableSource<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader().read(buf)
    }
}

impl<R: Read + Seek> Seek for SeekableSource<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.reader().seek(pos)
    }
}

impl<R: Read + Seek + Send> MediaSource for SeekableSource<R> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.length)
    }
}

/// A file read through symphonia.
///
/// Positions inside are in the stream's frames, which include any encoder
/// priming that symphonia doesn't trim itself (AAC in MPEG-4); `lead` is that
/// priming, and the frames callers see start after it.
struct SymphoniaStream {
    info: MediaInfo,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    /// Decoded frames not yet handed out, planar.
    pending: Vec<Vec<f32>>,
    /// The first frame in `pending` not yet handed out.
    pending_at: usize,
    /// The stream frame the next decoded packet starts at, or `None` after a
    /// seek until a packet's timestamp says.
    cursor: Option<u64>,
    /// Stream frames before this one are decoded and dropped: the priming, or
    /// the frames before a seek target.
    skip_before: u64,
    /// The stream frame where padding starts, if the file records it.
    end: Option<u64>,
    /// Priming frames at the start of the stream.
    lead: u64,
    /// Whether nothing more will be decoded.
    finished: bool,
    damaged: u64,
}

impl SymphoniaStream {
    fn open(
        source: Box<dyn MediaSource>,
        extension: Option<&str>,
        trim: Option<RecordedTrim>,
    ) -> Result<Self, MediaError> {
        let stream = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
        let mut hint = Hint::new();
        if let Some(extension) = extension {
            hint.with_extension(extension);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(from_symphonia)?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or(MediaError::NoAudio)?;
        let Some(CodecParameters::Audio(params)) = &track.codec_params else {
            return Err(MediaError::NoAudio);
        };
        let params: AudioCodecParameters = params.clone();
        let track_id = track.id;
        let time_base = track.time_base;
        let mut frames = track.num_frames;
        // Before the codec allocates for the header's channels and length.
        limits::check_header(
            params.channels.as_ref().map_or(0, |c| c.count() as u64),
            params.sample_rate.unwrap_or(0),
            frames,
        )?;

        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(from_symphonia)?;

        let sample_rate = params
            .sample_rate
            .filter(|&r| r > 0)
            .ok_or_else(|| MediaError::Malformed("no sample rate".into()))?;
        let channels = params
            .channels
            .as_ref()
            .map(|c| c.count())
            .ok_or_else(|| MediaError::Malformed("no channel count".into()))?;
        let channels = u16::try_from(channels)
            .ok()
            .filter(|&c| c > 0)
            .ok_or_else(|| MediaError::Malformed(format!("{channels} channels")))?;

        let codec = classify_codec(&params);
        let container = classify_container(format.as_ref());
        let bits_per_sample = if codec.is_lossless() {
            params.bits_per_sample
        } else {
            None
        };

        // Symphonia trims encoder delay and padding itself for MP3, Vorbis,
        // and the lossless codecs, but not for AAC in MPEG-4.
        let (lead, end) = match trim {
            Some(trim) if codec == Codec::Aac && container == Container::Mp4 => {
                let trim = trim.at_rate(sample_rate);
                let end = trim.valid.map(|v| trim.priming.checked_add(v));
                // A tag whose fields overflow is garbage: decode untrimmed.
                if end == Some(None) {
                    (0, None)
                } else {
                    frames = trim
                        .valid
                        .or_else(|| frames.map(|f| f.saturating_sub(trim.priming)));
                    (trim.priming, end.flatten())
                }
            }
            _ => (0, None),
        };

        let info = MediaInfo {
            container,
            codec,
            sample_rate,
            channels,
            frames,
            bits_per_sample,
        };

        Ok(Self {
            info,
            format,
            decoder,
            track_id,
            time_base,
            pending: vec![Vec::new(); usize::from(channels)],
            pending_at: 0,
            cursor: Some(0),
            skip_before: lead,
            end,
            lead,
            finished: false,
            damaged: 0,
        })
    }

    fn read(&mut self, out: &mut [&mut [f32]]) -> Result<usize, MediaError> {
        let wanted = out.iter().map(|c| c.len()).min().unwrap_or(0);
        let mut written = 0;
        while written < wanted {
            let available = self.pending[0].len() - self.pending_at;
            if available == 0 {
                if self.finished || !self.decode_next()? {
                    break;
                }
                continue;
            }
            let n = available.min(wanted - written);
            for (dst, src) in out.iter_mut().zip(&self.pending) {
                dst[written..written + n]
                    .copy_from_slice(&src[self.pending_at..self.pending_at + n]);
            }
            self.pending_at += n;
            written += n;
        }
        Ok(written)
    }

    fn seek(&mut self, frame: u64) -> Result<(), MediaError> {
        let target = frame.saturating_add(self.lead);
        self.pending.iter_mut().for_each(Vec::clear);
        self.pending_at = 0;
        let past_end = self.end.is_some_and(|end| target >= end)
            || self.info.frames.is_some_and(|frames| frame >= frames);
        if past_end {
            self.finished = true;
            return Ok(());
        }
        // A lossy decoder restarted mid-stream gets its first blocks wrong:
        // they overlap-add with blocks it never decoded, and MP3 frames can
        // borrow bits from earlier frames. So seek a little earlier and
        // decode up to the target.
        let preroll = match self.info.codec {
            Codec::Mp3 => 4 * 1152,
            Codec::Aac => 4 * 1024,
            Codec::Vorbis => 8192,
            _ => 0,
        };
        let seeked = self.format.seek(
            SeekMode::Accurate,
            SeekTo::Timestamp {
                ts: self.frame_to_ts(target.saturating_sub(preroll)),
                track_id: self.track_id,
            },
        );
        match seeked {
            Ok(_) => {
                self.decoder.reset();
                self.finished = false;
                self.cursor = None;
                self.skip_before = target;
                Ok(())
            }
            // Past the end of a file whose length the header didn't give:
            // there's nothing more to read.
            Err(SymphoniaError::SeekError(SeekErrorKind::OutOfRange)) => {
                self.finished = true;
                Ok(())
            }
            Err(SymphoniaError::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                self.finished = true;
                Ok(())
            }
            Err(e) => Err(from_symphonia(e)),
        }
    }

    /// Decodes packets until one leaves frames to hand out. Returns `false`
    /// at the end.
    fn decode_next(&mut self) -> Result<bool, MediaError> {
        while !self.finished {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => break,
                // A file cut short ends where its data ends.
                Err(SymphoniaError::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(e) => return Err(from_symphonia(e)),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            // The first frame left after symphonia's own trim.
            let packet_start = packet
                .pts
                .get()
                .saturating_add_unsigned(packet.trim_start.get());
            self.decode_packet(&packet)?;

            let frames = self.pending[0].len() as u64;
            if frames == 0 {
                // A decoder's first packet can yield nothing (Vorbis primes
                // its overlap with it), so it can't place the stream.
                continue;
            }
            let start = self
                .cursor
                .unwrap_or_else(|| self.ts_to_frame(Timestamp::new(packet_start)).unwrap_or(0));
            self.cursor = Some(start + frames);

            let low = self.skip_before.saturating_sub(start).min(frames);
            let high = self
                .end
                .map_or(frames, |end| end.saturating_sub(start).min(frames));
            if self.end.is_some_and(|end| start + frames >= end) {
                self.finished = true;
            }
            if high > low {
                for plane in &mut self.pending {
                    plane.truncate(high as usize);
                }
                self.pending_at = low as usize;
                return Ok(true);
            }
        }
        self.finished = true;
        Ok(false)
    }

    /// Decodes one packet into `pending`, replacing what was there.
    fn decode_packet(&mut self, packet: &Packet) -> Result<(), MediaError> {
        self.pending_at = 0;
        let channels = self.pending.len();
        match self.decoder.decode(packet) {
            Ok(buffer) => {
                let frames = buffer.frames();
                if buffer.num_planes() == channels {
                    for plane in &mut self.pending {
                        plane.resize(frames, 0.0);
                    }
                    buffer.copy_to_slice_planar(&mut self.pending);
                } else {
                    // A stream that changes its channel count mid-way keeps
                    // the first layout: extra channels are dropped and
                    // missing ones are silent.
                    let mut planes = Vec::new();
                    buffer.copy_to_vecs_planar::<f32>(&mut planes);
                    for (i, plane) in self.pending.iter_mut().enumerate() {
                        plane.clear();
                        match planes.get(i) {
                            Some(src) => plane.extend_from_slice(src),
                            None => plane.resize(frames, 0.0),
                        }
                    }
                }
                Ok(())
            }
            Err(SymphoniaError::DecodeError(_)) => {
                // Keep time: a damaged packet becomes silence.
                self.damaged += 1;
                let frames = self.dur_to_frames(packet.dur.get()) as usize;
                for plane in &mut self.pending {
                    plane.clear();
                    plane.resize(frames, 0.0);
                }
                Ok(())
            }
            Err(e) => Err(from_symphonia(e)),
        }
    }

    fn frame_to_ts(&self, frame: u64) -> Timestamp {
        let ts = match self.time_base {
            Some(tb) if !is_recip(tb, self.info.sample_rate) => {
                let numer = u128::from(tb.numer.get()) * u128::from(self.info.sample_rate);
                (u128::from(frame) * u128::from(tb.denom.get()) / numer) as i64
            }
            _ => frame as i64,
        };
        Timestamp::new(ts)
    }

    /// The frame a timestamp falls on, or `None` before the start.
    fn ts_to_frame(&self, ts: Timestamp) -> Option<u64> {
        let ts = u64::try_from(ts.get()).ok()?;
        Some(self.dur_to_frames(ts))
    }

    fn dur_to_frames(&self, dur: u64) -> u64 {
        match self.time_base {
            Some(tb) if !is_recip(tb, self.info.sample_rate) => {
                let numer = u128::from(tb.numer.get()) * u128::from(self.info.sample_rate);
                (u128::from(dur) * numer / u128::from(tb.denom.get())) as u64
            }
            _ => dur,
        }
    }
}

fn is_recip(tb: TimeBase, rate: u32) -> bool {
    tb.numer.get() == 1 && tb.denom.get() == rate
}

fn classify_codec(params: &AudioCodecParameters) -> Codec {
    let id = params.codec;
    if (codec_ids::CODEC_ID_PCM_S32LE..=codec_ids::CODEC_ID_PCM_MULAW).contains(&id) {
        Codec::Pcm
    } else if (codec_ids::CODEC_ID_ADPCM_G722..codec_ids::CODEC_ID_VORBIS).contains(&id) {
        Codec::Adpcm
    } else if id == codec_ids::CODEC_ID_FLAC {
        Codec::Flac
    } else if id == codec_ids::CODEC_ID_ALAC {
        Codec::Alac
    } else if id == codec_ids::CODEC_ID_MP3 {
        Codec::Mp3
    } else if id == codec_ids::CODEC_ID_VORBIS {
        Codec::Vorbis
    } else if id == codec_ids::CODEC_ID_AAC {
        Codec::Aac
    } else {
        Codec::Other
    }
}

fn classify_container(format: &dyn FormatReader) -> Container {
    match format.format_info().format {
        format_ids::FORMAT_ID_WAVE => Container::Wav,
        format_ids::FORMAT_ID_AIFF => Container::Aiff,
        format_ids::FORMAT_ID_FLAC => Container::Flac,
        format_ids::FORMAT_ID_MP3 => Container::Mp3,
        format_ids::FORMAT_ID_OGG => Container::Ogg,
        format_ids::FORMAT_ID_ISOMP4 => Container::Mp4,
        format_ids::FORMAT_ID_ADTS => Container::Adts,
        _ => Container::Other,
    }
}

fn read_up_to(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// `fLaC`, a last STREAMINFO block with `min` and `max` block sizes,
    /// and a frame header's first two bytes.
    fn flac_head(min: u16, max: u16, frame: [u8; 2]) -> Vec<u8> {
        let mut bytes = b"fLaC".to_vec();
        bytes.extend([0x80, 0, 0, 34]);
        let mut info = [0; 34];
        info[0..2].copy_from_slice(&min.to_be_bytes());
        info[2..4].copy_from_slice(&max.to_be_bytes());
        bytes.extend(info);
        bytes.extend(frame);
        bytes
    }

    const FIXED: [u8; 2] = [0xff, 0xf8];
    const VARIABLE: [u8; 2] = [0xff, 0xf9];

    #[test]
    fn only_a_fixed_blocksize_stream_with_a_smaller_minimum_is_the_quirk() {
        let quirk = |bytes: Vec<u8>| fixed_block_size_quirk(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(quirk(flac_head(1792, 4096, FIXED)), Some(4096));
        assert_eq!(quirk(flac_head(4096, 4096, FIXED)), None);
        assert_eq!(quirk(flac_head(1792, 4096, VARIABLE)), None);
        assert_eq!(quirk(b"RIFF\0\0\0\0WAVEfmt ".to_vec()), None);
        assert_eq!(quirk(b"fLaC".to_vec()), None);
    }

    #[test]
    fn the_fix_reads_the_fixed_minimum_however_the_reads_fall() {
        let bytes = flac_head(1792, 4096, FIXED);
        let mut expected = bytes.clone();
        expected[8..10].copy_from_slice(&4096u16.to_be_bytes());
        for chunk in [1, 3, 9, bytes.len()] {
            let mut fix = FlacBlockSizeFix::new(Cursor::new(bytes.clone()), 4096);
            let mut read: Vec<u8> = Vec::new();
            let mut buf = vec![0; chunk];
            loop {
                let n = fix.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                read.extend(&buf[..n]);
            }
            assert_eq!(read, expected, "reads of {chunk}");
        }
        // After a seek past the field, nothing else changes.
        let mut fix = FlacBlockSizeFix::new(Cursor::new(bytes.clone()), 4096);
        fix.seek(SeekFrom::Start(10)).unwrap();
        let mut rest = Vec::new();
        fix.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, bytes[10..]);
    }
}
