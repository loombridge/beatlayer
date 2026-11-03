//! Create Video (the export spec, R46 to R52): a QuickTime movie written
//! beside an audio file, its frames the Arrangement's picture at each frame
//! time of the range and its sound the rendered audio, read back from the
//! finished audio file so Normalize, Convert to Mono and Render as Loop are
//! all in it.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bl_core::time::Ticks;
use bl_media::Decoder;
use bl_model::{ClipData, ClipId, ContentHash, Document};
use bl_shell_spec::editing::Position;
use bl_shell_spec::export::{ExportArgs, VideoEncoder, VideoSettings};
use bl_video::{
    AudioFormat, CodedFrame, EncoderKind, EncoderSettings, Frame, FrameSource, Movie, MovieWriter,
    Picture, Rational, VideoFormat, available_encoders, encoded_size, frame_rate_of, open_encoder,
    scale,
};

use crate::export::ExportError;

/// The movie a video clip names, if its file can be read.
pub type MovieOf<'a> = dyn Fn(ContentHash) -> Option<Arc<Movie>> + 'a;

/// The spec's name for an encoder, as `bl-video` has it.
fn kind_of(encoder: VideoEncoder) -> EncoderKind {
    match encoder {
        VideoEncoder::H264 => EncoderKind::H264,
        VideoEncoder::Hevc => EncoderKind::Hevc,
        VideoEncoder::ProRes422 => EncoderKind::ProRes422,
        VideoEncoder::MotionJpeg => EncoderKind::MotionJpeg,
    }
}

fn encoder_of(kind: EncoderKind) -> VideoEncoder {
    match kind {
        EncoderKind::H264 => VideoEncoder::H264,
        EncoderKind::Hevc => VideoEncoder::Hevc,
        EncoderKind::ProRes422 => VideoEncoder::ProRes422,
        EncoderKind::MotionJpeg => VideoEncoder::MotionJpeg,
    }
}

/// The encoders this computer lists, in the chooser's order (R47).
#[must_use]
pub fn listed_encoders() -> Vec<VideoEncoder> {
    available_encoders().into_iter().map(encoder_of).collect()
}

/// The encoder a left-out choice means: H.264 where the computer has it,
/// else Motion JPEG (R47).
#[must_use]
pub fn default_encoder() -> VideoEncoder {
    let listed = listed_encoders();
    if listed.contains(&VideoEncoder::H264) {
        VideoEncoder::H264
    } else {
        VideoEncoder::MotionJpeg
    }
}

/// The encoder to use for a choice, or why there is none.
///
/// # Errors
///
/// A message naming the encoders this computer lists.
pub fn choose_encoder(asked: Option<VideoEncoder>) -> Result<EncoderKind, String> {
    let encoder = asked.unwrap_or_else(default_encoder);
    let listed = listed_encoders();
    if listed.contains(&encoder) {
        return Ok(kind_of(encoder));
    }
    let names: Vec<&str> = listed.iter().map(|e| e.name()).collect();
    Err(format!(
        "this computer has no {} encoder; it has {}",
        encoder.name(),
        names.join(", ")
    ))
}

/// A frame rate from the number a person gave: the exact rate for the
/// usual ones, so 29.97 is 30000/1001, else thousandths of a frame.
fn rate_of(fps: f64) -> Rational {
    const USUAL: [(i128, i128); 8] = [
        (24_000, 1001),
        (24, 1),
        (25, 1),
        (30_000, 1001),
        (30, 1),
        (50, 1),
        (60_000, 1001),
        (60, 1),
    ];
    USUAL
        .iter()
        .find(|&&(n, d)| (fps - n as f64 / d as f64).abs() < 0.005)
        .map_or_else(
            || Rational::new((fps * 1000.0).round() as i128, 1000),
            |&(n, d)| Rational::new(n, d),
        )
}

/// Whether the settings can be used.
///
/// # Errors
///
/// A message naming what is wrong.
pub fn check_settings(settings: &VideoSettings) -> Result<(), String> {
    if let Some(quality) = settings.quality
        && !(1..=100).contains(&quality)
    {
        return Err(format!("a video quality of {quality}; give 1 to 100"));
    }
    if let Some(fps) = settings.frame_rate
        && !(1.0..=240.0).contains(&fps)
    {
        return Err(format!("a frame rate of {fps}; give 1 to 240"));
    }
    if let Some(size) = settings.size
        && (size.width < 2 || size.height < 2 || size.width > 8192 || size.height > 8192)
    {
        return Err(format!(
            "a video size of {} by {}; give 2 to 8192 pixels each way",
            size.width, size.height
        ));
    }
    Ok(())
}

/// The range of a video clip's whole movie, with its pre-roll (R50), as
/// `start` and `length` in ticks. A movie that starts before 1.1.1 starts
/// there.
///
/// # Errors
///
/// A message if the clip isn't a video clip, its movie can't be read, or
/// the movie ends before the song begins.
pub fn clip_range(
    document: &Document,
    clip: ClipId,
    movies: &MovieOf<'_>,
) -> Result<(Ticks, Ticks), String> {
    let found = document
        .clip(clip)
        .ok_or_else(|| format!("no clip has the ID {}", clip.0))?;
    let ClipData::Audio(audio) = &found.data else {
        return Err(format!("clip {} isn't a video clip", clip.0));
    };
    let movie = audio
        .video
        .then(|| movies(audio.media))
        .flatten()
        .ok_or_else(|| format!("clip {} isn't a video clip whose movie can be read", clip.0))?;
    let (from, to) = found
        .movie_span(&document.tempo_map(), movie.duration().seconds())
        .ok_or_else(|| format!("clip {} isn't a video clip", clip.0))?;
    let from = from.max(Ticks::ZERO);
    if to <= from {
        return Err("the clip's movie ends before the song begins".to_owned());
    }
    Ok((from, to - from))
}

/// `args` with the range a `video.clip` gives when `start` and `length` are
/// left out (R50, R52).
///
/// # Errors
///
/// As [`clip_range`].
pub fn with_clip_range(
    document: &Document,
    args: &ExportArgs,
    movies: &MovieOf<'_>,
) -> Result<ExportArgs, String> {
    let mut args = args.clone();
    if let Some(clip) = args.video.as_ref().and_then(|video| video.clip)
        && args.start.is_none()
        && args.length.is_none()
    {
        let (start, length) = clip_range(document, clip, movies)?;
        args.start = Some(Position::Ticks(start));
        args.length = Some(length);
    }
    Ok(args)
}

/// What one movie is written from.
pub struct MovieJob<'a> {
    /// The set as the Arrangement has it: the video clips of every track.
    pub document: &'a Document,
    /// Where the video clips' movies are.
    pub movies: &'a MovieOf<'a>,
    /// Where the range starts.
    pub start: Ticks,
    /// The audio file, whole, under whatever name, and its extension.
    pub audio: &'a Path,
    /// The audio file's extension, which tells the decoder its format.
    pub audio_extension: &'static str,
    /// The encoder.
    pub encoder: EncoderKind,
    /// The encoder's settings.
    pub settings: &'a VideoSettings,
    /// The file to write.
    pub out: &'a Path,
}

/// Writes the movie (R46, R49, R51).
///
/// `progress` is told the share done, from 0 to 1; `cancel` is looked at
/// between frames.
///
/// # Errors
///
/// [`ExportError`]: cancelled, or a file or an encoder that failed. The
/// partial file is left for the caller to remove.
pub fn write_movie(
    job: &MovieJob<'_>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<(), ExportError> {
    let failed = |what: &dyn std::fmt::Display| ExportError::Failed(format!("the movie: {what}"));
    let document = job.document;
    let tempo = document.tempo_map();

    let mut audio = Decoder::from_reader(
        BufReader::new(File::open(job.audio).map_err(|e| failed(&e))?),
        Some(job.audio_extension),
    )
    .map_err(|e| failed(&e))?;
    let (rate, channels) = (audio.info().sample_rate, usize::from(audio.info().channels));
    let audio_frames = audio
        .info()
        .frames
        .ok_or_else(|| failed(&"the audio file doesn't say how long it is"))?;

    // The source movie's rate and size are the movie's own unless changed
    // (R48): the first video clip in the range.
    let seconds = audio_frames as f64 / f64::from(rate);
    let end = tempo.seconds_to_ticks(tempo.ticks_to_seconds(job.start) + seconds);
    let source = document
        .track_order()
        .into_iter()
        .filter_map(|id| document.track(id))
        .flat_map(|track| track.clips())
        .filter(|clip| !clip.mute && clip.start < end && job.start < clip.end())
        .find_map(|clip| match &clip.data {
            ClipData::Audio(audio) if audio.video => (job.movies)(audio.media),
            _ => None,
        });
    let track = source.as_ref().and_then(|movie| movie.video());
    let fps = job
        .settings
        .frame_rate
        .map(rate_of)
        .or_else(|| track.and_then(frame_rate_of))
        .unwrap_or_else(|| Rational::new(30, 1));
    let (width, height) = job.settings.size.map_or_else(
        || track.map_or((1280, 720), |t| (t.width(), t.height())),
        |size| (size.width, size.height),
    );
    let (width, height) = encoded_size(width, height);
    let fps_value = fps.seconds();

    let mut encoder = open_encoder(
        job.encoder,
        width,
        height,
        fps,
        &EncoderSettings {
            quality: job.settings.quality(),
            frame_rate: None,
            size: None,
        },
    )
    .map_err(|e| failed(&e))?;
    let mut writer = MovieWriter::new(
        BufWriter::with_capacity(1 << 20, File::create(job.out).map_err(|e| failed(&e))?),
        VideoFormat {
            width,
            height,
            frame_rate: fps,
        },
        AudioFormat {
            sample_rate: rate,
            channels: channels as u16,
        },
    )
    .map_err(|e| failed(&e))?;

    // A frame for every `1 / fps` of the audio, the last one the audio's
    // end falls in; a hair under a whole frame doesn't add one.
    let exact = seconds * fps_value;
    let count = if (exact - exact.round()).abs() < 0.01 {
        exact.round()
    } else {
        exact.ceil()
    } as u64;
    if count == 0 {
        return Err(ExportError::Invalid(
            "the range is too short to hold a frame of video".to_owned(),
        ));
    }

    let black = Frame::new(
        width,
        height,
        [0, 0, 0, 255].repeat(width as usize * height as usize),
    )
    .ok_or_else(|| failed(&"a picture too large"))?;
    let mut sources: HashMap<ContentHash, Option<(Arc<Movie>, FrameSource)>> = HashMap::new();
    let mut coded: Vec<CodedFrame> = Vec::new();
    let mut planes = vec![Vec::<f32>::new(); channels];
    let mut interleaved: Vec<f32> = Vec::new();
    let mut audio_done = 0u64;
    let start_seconds = tempo.ticks_to_seconds(job.start);

    for k in 0..count {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        // Spec: export R49. The picture the Video Window shows at the song
        // time of the frame: sampled in the middle of the frame's interval,
        // so that rounding to a tick never lands a frame on the one before.
        let at = tempo.seconds_to_ticks(start_seconds + (k as f64 + 0.5) / fps_value);
        let picture = picture_at(document, &tempo, at, job.movies, &mut sources);
        let frame = picture.map_or_else(|| black.clone(), |frame| scale(&frame, width, height));
        coded.clear();
        encoder.encode(&frame, &mut coded).map_err(|e| failed(&e))?;
        for frame in &coded {
            writer.write_video(frame).map_err(|e| failed(&e))?;
        }
        // The audio to the end of this frame's interval, so the file holds
        // picture and sound in the order they play.
        let until = if k + 1 == count {
            audio_frames
        } else {
            (((k + 1) as f64 / fps_value * f64::from(rate)).ceil() as u64).min(audio_frames)
        };
        write_audio(
            &mut audio,
            &mut writer,
            &mut planes,
            &mut interleaved,
            until.saturating_sub(audio_done) as usize,
        )
        .map_err(|e| failed(&e))?;
        audio_done = audio_done.max(until);
        progress((k + 1) as f64 / count as f64);
    }
    coded.clear();
    encoder.finish(&mut coded).map_err(|e| failed(&e))?;
    for frame in &coded {
        writer.write_video(frame).map_err(|e| failed(&e))?;
    }
    let entry = encoder.entry().map_err(|e| failed(&e))?;
    let mut out = writer.finish(&entry).map_err(|e| failed(&e))?;
    out.flush().map_err(|e| failed(&e))?;
    Ok(())
}

/// The picture of the video clip that plays at `at`, if one plays and its
/// movie shows a frame there.
fn picture_at(
    document: &Document,
    tempo: &bl_core::time::TempoMap,
    at: Ticks,
    movies: &MovieOf<'_>,
    sources: &mut HashMap<ContentHash, Option<(Arc<Movie>, FrameSource)>>,
) -> Option<Arc<Frame>> {
    let clip = document.video_clip_at(at)?;
    let ClipData::Audio(audio) = &clip.data else {
        return None;
    };
    let source = sources.entry(audio.media).or_insert_with(|| {
        let movie = movies(audio.media)?;
        let frames = FrameSource::new(Arc::clone(&movie)).ok()?;
        Some((movie, frames))
    });
    let (movie, frames) = source.as_mut()?;
    let movie_time = clip.movie_time_at(at, tempo, movie.duration().seconds())?;
    // Nanoseconds, as `video.frame_at` does, so a time a warp map puts
    // between two frames' starts lands on the frame it is in.
    match frames.picture_at(Rational::new(
        (movie_time * 1e9).round() as i128,
        1_000_000_000,
    )) {
        Picture::Frame(frame) => Some(frame),
        Picture::Black | Picture::Pending => None,
    }
}

/// Reads `frames` frames of the audio file and writes them as a chunk.
fn write_audio<W: Write + std::io::Seek>(
    audio: &mut Decoder,
    writer: &mut MovieWriter<W>,
    planes: &mut [Vec<f32>],
    interleaved: &mut Vec<f32>,
    frames: usize,
) -> Result<(), String> {
    if frames == 0 {
        return Ok(());
    }
    for plane in planes.iter_mut() {
        plane.clear();
        plane.resize(frames, 0.0);
    }
    let mut got = 0;
    while got < frames {
        let mut slices: Vec<&mut [f32]> = planes.iter_mut().map(|p| &mut p[got..]).collect();
        let read = audio.read(&mut slices).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        got += read;
    }
    interleaved.clear();
    for i in 0..got {
        interleaved.extend(planes.iter().map(|p| p[i]));
    }
    writer.write_audio(interleaved).map_err(|e| e.to_string())
}
