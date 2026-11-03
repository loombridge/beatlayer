//! A QuickTime movie written as it is encoded (the export spec, R46, R51):
//! the `mdat` first, with the picture's frames and the sound's chunks in the
//! order they are given, then the `moov` that places them. The picture is
//! whatever an encoder codes; the sound is 24-bit PCM.

use std::io::{self, Seek, SeekFrom, Write};

use crate::encoder::{CodedFrame, SampleEntry};
use crate::rational::Rational;

/// The picture's time scale is at least this, so a whole frame is a whole
/// number of its units.
const MIN_VIDEO_SCALE: u32 = 600;

/// The picture's size and rate.
#[derive(Clone, Copy, Debug)]
pub struct VideoFormat {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frames a second.
    pub frame_rate: Rational,
}

/// The sound: 24-bit PCM, big-endian.
#[derive(Clone, Copy, Debug)]
pub struct AudioFormat {
    /// Frames a second.
    pub sample_rate: u32,
    /// Channels in each frame.
    pub channels: u16,
}

/// Writes a movie to `W`. Call [`MovieWriter::write_video`] and
/// [`MovieWriter::write_audio`] as the data comes, in the order it should
/// sit in the file, then [`MovieWriter::finish`].
pub struct MovieWriter<W: Write + Seek> {
    out: W,
    video: VideoFormat,
    audio: AudioFormat,
    /// Where the `mdat`'s size is, to write when the movie ends.
    mdat_at: u64,
    position: u64,
    /// Each coded frame: its offset, size and whether it is a sync frame.
    frames: Vec<(u64, u32, bool)>,
    /// Each sound chunk: its offset and its frames.
    chunks: Vec<(u64, u32)>,
    audio_frames: u64,
    scratch: Vec<u8>,
}

impl<W: Write + Seek> MovieWriter<W> {
    /// Starts a movie: the file type and the `mdat`'s header go out now.
    ///
    /// # Errors
    ///
    /// Any write error.
    pub fn new(mut out: W, video: VideoFormat, audio: AudioFormat) -> io::Result<Self> {
        out.seek(SeekFrom::Start(0))?;
        let mut head = atom(
            b"ftyp",
            &[&b"qt  "[..], &0x200u32.to_be_bytes(), b"qt  "].concat(),
        );
        let mdat_at = head.len() as u64;
        // An `mdat` of 64-bit size: the movie may pass 4 GB.
        head.extend_from_slice(&1u32.to_be_bytes());
        head.extend_from_slice(b"mdat");
        head.extend_from_slice(&0u64.to_be_bytes());
        out.write_all(&head)?;
        Ok(Self {
            out,
            video,
            audio,
            mdat_at,
            position: head.len() as u64,
            frames: Vec::new(),
            chunks: Vec::new(),
            audio_frames: 0,
            scratch: Vec::new(),
        })
    }

    /// Appends a coded frame.
    ///
    /// # Errors
    ///
    /// Any write error.
    pub fn write_video(&mut self, frame: &CodedFrame) -> io::Result<()> {
        self.frames
            .push((self.position, frame.data.len() as u32, frame.sync));
        self.out.write_all(&frame.data)?;
        self.position += frame.data.len() as u64;
        Ok(())
    }

    /// Appends sound: interleaved frames, rounded to 24 bits.
    ///
    /// # Errors
    ///
    /// Any write error.
    pub fn write_audio(&mut self, samples: &[f32]) -> io::Result<()> {
        let channels = usize::from(self.audio.channels).max(1);
        let frames = samples.len() / channels;
        if frames == 0 {
            return Ok(());
        }
        self.scratch.clear();
        for &sample in &samples[..frames * channels] {
            let value = (f64::from(sample) * 8_388_608.0)
                .round()
                .clamp(-8_388_608.0, 8_388_607.0) as i32;
            self.scratch.extend_from_slice(&value.to_be_bytes()[1..]);
        }
        self.chunks.push((self.position, frames as u32));
        self.out.write_all(&self.scratch)?;
        self.position += self.scratch.len() as u64;
        self.audio_frames += frames as u64;
        Ok(())
    }

    /// Ends the movie: sizes the `mdat` and writes the `moov` after it.
    /// Returns the writer's destination.
    ///
    /// # Errors
    ///
    /// Any write error.
    pub fn finish(mut self, entry: &SampleEntry) -> io::Result<W> {
        let moov = self.moov(entry);
        self.out.write_all(&moov)?;
        let size = self.position - self.mdat_at;
        self.out.seek(SeekFrom::Start(self.mdat_at + 8))?;
        self.out.write_all(&size.to_be_bytes())?;
        self.out.seek(SeekFrom::End(0))?;
        self.out.flush()?;
        Ok(self.out)
    }

    /// The picture's time scale and how long a frame is in it.
    fn video_time(&self) -> (u32, u32) {
        let rate = self.video.frame_rate;
        let (num, den) = (rate.numer().max(1) as u64, rate.denom().max(1));
        let multiple = u64::from(MIN_VIDEO_SCALE).div_ceil(num).max(1);
        ((num * multiple) as u32, (den * multiple) as u32)
    }

    fn moov(&self, entry: &SampleEntry) -> Vec<u8> {
        let (video_scale, frame) = self.video_time();
        let video_media = u64::from(frame) * self.frames.len() as u64;
        let audio_media = self.audio_frames;
        // The movie counts in the sound's frames, so a track's default edit
        // plays every one of them.
        let movie_scale = self.audio.sample_rate;
        let to_movie = |units: u64, scale: u64| (units * u64::from(movie_scale)).div_ceil(scale);
        let video_movie = to_movie(video_media, u64::from(video_scale));
        let audio_movie = to_movie(audio_media, u64::from(self.audio.sample_rate));
        let duration = video_movie.max(audio_movie);

        let mvhd = full(
            b"mvhd",
            0,
            0,
            &[
                &[0u8; 8][..],
                &movie_scale.to_be_bytes(),
                &(duration as u32).to_be_bytes(),
                &0x10000u32.to_be_bytes(),
                &0x100u16.to_be_bytes(),
                &[0u8; 10],
                &MATRIX,
                &[0u8; 24],
                &3u32.to_be_bytes(),
            ]
            .concat(),
        );
        let video = self.video_track(entry, video_scale, frame, video_media, video_movie);
        let audio = self.audio_track(audio_media, audio_movie);
        atom(b"moov", &[mvhd, video, audio].concat())
    }

    fn video_track(
        &self,
        entry: &SampleEntry,
        scale: u32,
        frame: u32,
        media: u64,
        movie: u64,
    ) -> Vec<u8> {
        let count = self.frames.len() as u32;
        let mut stbl = full(
            b"stts",
            0,
            0,
            &[1u32, count, frame]
                .iter()
                .flat_map(|v| v.to_be_bytes())
                .collect::<Vec<_>>(),
        );
        if self.frames.iter().any(|f| !f.2) {
            let syncs: Vec<u32> = (0..count).filter(|&i| self.frames[i as usize].2).collect();
            stbl.extend(full(
                b"stss",
                0,
                0,
                &[
                    (syncs.len() as u32).to_be_bytes().to_vec(),
                    syncs.iter().flat_map(|i| (i + 1).to_be_bytes()).collect(),
                ]
                .concat(),
            ));
        }
        stbl.extend(full(
            b"stsc",
            0,
            0,
            &[1u32, 1, 1, 1]
                .iter()
                .flat_map(|v| v.to_be_bytes())
                .collect::<Vec<_>>(),
        ));
        let sizes: Vec<u8> = self.frames.iter().flat_map(|f| f.1.to_be_bytes()).collect();
        stbl.extend(full(
            b"stsz",
            0,
            0,
            &[&0u32.to_be_bytes()[..], &count.to_be_bytes(), &sizes].concat(),
        ));
        stbl.extend(offsets(self.frames.iter().map(|f| f.0)));
        let stbl = atom(
            b"stbl",
            &[
                full(
                    b"stsd",
                    0,
                    0,
                    &[&1u32.to_be_bytes()[..], &self.video_entry(entry)].concat(),
                ),
                stbl,
            ]
            .concat(),
        );
        track(
            1,
            b"vide",
            scale,
            media,
            movie,
            (self.video.width, self.video.height),
            &full(b"vmhd", 0, 1, &[0u8; 8]),
            &stbl,
        )
    }

    fn video_entry(&self, entry: &SampleEntry) -> Vec<u8> {
        let mut body = [0u8; 6].to_vec();
        body.extend_from_slice(&1u16.to_be_bytes());
        // Version, revision, vendor, temporal and spatial quality.
        body.extend_from_slice(&[0u8; 16]);
        body.extend_from_slice(&(self.video.width as u16).to_be_bytes());
        body.extend_from_slice(&(self.video.height as u16).to_be_bytes());
        // 72 dpi each way, no data size, one frame a sample.
        body.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        body.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&1u16.to_be_bytes());
        // The compressor's name: empty.
        body.extend_from_slice(&[0u8; 32]);
        // Depth 24, and no colour table.
        body.extend_from_slice(&24u16.to_be_bytes());
        body.extend_from_slice(&(-1i16).to_be_bytes());
        for (kind, contents) in &entry.extra {
            body.extend(atom(kind, contents));
        }
        atom(&entry.fourcc, &body)
    }

    fn audio_track(&self, media: u64, movie: u64) -> Vec<u8> {
        let mut stbl = full(
            b"stts",
            0,
            0,
            &[1u32, media as u32, 1]
                .iter()
                .flat_map(|v| v.to_be_bytes())
                .collect::<Vec<_>>(),
        );
        // Runs of chunks that hold the same number of frames.
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for (i, &(_, frames)) in self.chunks.iter().enumerate() {
            if runs.last().is_none_or(|run| run.1 != frames) {
                runs.push((i as u32 + 1, frames));
            }
        }
        stbl.extend(full(
            b"stsc",
            0,
            0,
            &[
                (runs.len() as u32).to_be_bytes().to_vec(),
                runs.iter()
                    .flat_map(|&(first, frames)| {
                        [first, frames, 1].into_iter().flat_map(u32::to_be_bytes)
                    })
                    .collect(),
            ]
            .concat(),
        ));
        let frame_bytes = u32::from(self.audio.channels) * 3;
        stbl.extend(full(
            b"stsz",
            0,
            0,
            &[frame_bytes, media as u32]
                .iter()
                .flat_map(|v| v.to_be_bytes())
                .collect::<Vec<_>>(),
        ));
        stbl.extend(offsets(self.chunks.iter().map(|c| c.0)));
        // A version 2 sound description: linear PCM with its format in
        // flags, which holds any rate and says the 24-bit samples are signed
        // and big-endian (a version 0 `in24` is read as little-endian by
        // some players).
        let mut body = [0u8; 6].to_vec();
        body.extend_from_slice(&1u16.to_be_bytes());
        // Version 2, revision and vendor 0.
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&[0u8; 6]);
        // The constants a version 2 description starts with.
        body.extend_from_slice(&3u16.to_be_bytes());
        body.extend_from_slice(&16u16.to_be_bytes());
        body.extend_from_slice(&(-2i16).to_be_bytes());
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        // The structure's size, the rate, the channels, and the constant.
        body.extend_from_slice(&72u32.to_be_bytes());
        body.extend_from_slice(&f64::from(self.audio.sample_rate).to_be_bytes());
        body.extend_from_slice(&u32::from(self.audio.channels).to_be_bytes());
        body.extend_from_slice(&0x7f00_0000u32.to_be_bytes());
        // 24 bits, signed, big-endian and packed; a frame's bytes, and one
        // frame to a packet.
        body.extend_from_slice(&24u32.to_be_bytes());
        body.extend_from_slice(&0xeu32.to_be_bytes());
        body.extend_from_slice(&(u32::from(self.audio.channels) * 3).to_be_bytes());
        body.extend_from_slice(&1u32.to_be_bytes());
        let stsd = full(
            b"stsd",
            0,
            0,
            &[&1u32.to_be_bytes()[..], &atom(b"lpcm", &body)].concat(),
        );
        let stbl = atom(b"stbl", &[stsd, stbl].concat());
        track(
            2,
            b"soun",
            self.audio.sample_rate,
            media,
            movie,
            (0, 0),
            &full(b"smhd", 0, 0, &[0u8; 4]),
            &stbl,
        )
    }
}

const MATRIX: [u8; 36] = {
    let words: [u32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];
    let mut bytes = [0u8; 36];
    let mut i = 0;
    while i < 9 {
        let b = words[i].to_be_bytes();
        bytes[i * 4] = b[0];
        bytes[i * 4 + 1] = b[1];
        bytes[i * 4 + 2] = b[2];
        bytes[i * 4 + 3] = b[3];
        i += 1;
    }
    bytes
};

fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let head = (u32::from(version) << 24 | flags).to_be_bytes();
    atom(kind, &[&head[..], body].concat())
}

/// The chunk offset table: 32-bit unless an offset needs more.
fn offsets(offsets: impl Iterator<Item = u64> + Clone) -> Vec<u8> {
    let count = offsets.clone().count() as u32;
    if offsets.clone().all(|o| o <= u64::from(u32::MAX)) {
        full(
            b"stco",
            0,
            0,
            &[
                count.to_be_bytes().to_vec(),
                offsets.flat_map(|o| (o as u32).to_be_bytes()).collect(),
            ]
            .concat(),
        )
    } else {
        full(
            b"co64",
            0,
            0,
            &[
                count.to_be_bytes().to_vec(),
                offsets.flat_map(u64::to_be_bytes).collect(),
            ]
            .concat(),
        )
    }
}

#[expect(clippy::too_many_arguments, reason = "a track box's fields")]
fn track(
    id: u32,
    handler: &[u8; 4],
    scale: u32,
    media: u64,
    movie: u64,
    size: (u32, u32),
    media_header: &[u8],
    stbl: &[u8],
) -> Vec<u8> {
    let tkhd = full(
        b"tkhd",
        0,
        3,
        &[
            &[0u8; 8][..],
            &id.to_be_bytes(),
            &[0u8; 4],
            &(movie as u32).to_be_bytes(),
            &[0u8; 8],
            &[0u8; 4],
            &(if handler == b"soun" { 0x100u16 } else { 0 }).to_be_bytes(),
            &[0u8; 2],
            &MATRIX,
            &(size.0 << 16).to_be_bytes(),
            &(size.1 << 16).to_be_bytes(),
        ]
        .concat(),
    );
    let mdhd = full(
        b"mdhd",
        0,
        0,
        &[
            &[0u8; 8][..],
            &scale.to_be_bytes(),
            &(media as u32).to_be_bytes(),
            &0x55c4u16.to_be_bytes(),
            &[0u8; 2],
        ]
        .concat(),
    );
    let hdlr = full(
        b"hdlr",
        0,
        0,
        &[&[0u8; 4][..], handler, &[0u8; 13]].concat(),
    );
    let dinf = atom(
        b"dinf",
        &full(
            b"dref",
            0,
            0,
            &[&1u32.to_be_bytes()[..], &full(b"url ", 0, 1, &[])].concat(),
        ),
    );
    let minf = atom(b"minf", &[media_header, &dinf, stbl].concat());
    atom(
        b"trak",
        &[tkhd, atom(b"mdia", &[mdhd, hdlr, minf].concat())].concat(),
    )
}
