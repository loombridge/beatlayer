//! Exporting audio (WP-1.12): a range of the set rendered offline through
//! the engine and written to a WAV, AIFF, or FLAC file. The behaviour it
//! follows is in the export spec.
//!
//! [`export`] runs one export on the calling thread:
//!
//! 1. The engine renders the range with [`bl_engine::Offline`], the code
//!    behind `bl-cli render` and the golden tests, which runs the same
//!    processor a device's audio thread runs, block after block, as fast as
//!    it can. The song plays from the range's start, as Play does from an
//!    insert marker there, with the Arrangement loop off.
//! 2. At a file rate below the engine's, the render is converted down with
//!    the media converter; otherwise the engine renders at the file's rate.
//! 3. With Normalize, the render goes to a scratch file first, so the gain
//!    can be worked out from its peak and applied on a second pass.
//! 4. [`bl_media::AudioWriter`] dithers, rounds, and encodes it, into a
//!    hidden file beside the destination that's moved into place only when
//!    it's complete. Cancelling or a failure deletes what was written and
//!    leaves the destination as it was.
//!
//! [`Shell`](crate::Shell) runs `export.audio` through it, with its set,
//! its media store, and the audio device's rate.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bl_core::time::{SampleRate, Ticks, TimeSignatureMap};
use bl_engine::{EngineConfig, MediaLibrary, Offline, Output, Registry, TransportCommand, When};
use bl_media::{AudioWriter, BitDepth, Dither, EncodeSpec, FileType, RateConverter};
use bl_model::{ContentHash, DeviceId, Document};
use bl_shell_spec::editing::Position;
use bl_shell_spec::export::{
    AUDIO, Dither as SpecDither, ExportArgs, ExportResult, FileType as SpecFileType, RangeSource,
};

use crate::ShellError;

/// The engine's rate when no audio device or setting gives one.
pub const DEFAULT_ENGINE_RATE: u32 = 48_000;

/// The block size an export renders in. The output is the same at any size
/// (the export spec, R14).
pub const BLOCK: usize = 512;

/// The lowest and highest rates an export takes.
pub const RATES: std::ops::RangeInclusive<u32> = 8_000..=384_000;

/// The longest export: a day.
const MAX_SECONDS: f64 = 86_400.0;

/// How long the range is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Length {
    /// To a position in the song: the first one not exported.
    To(Ticks),
    /// This many frames, at the rate the engine renders at. Golden tests
    /// and previews use it for lengths that aren't positions.
    Frames(u64),
}

/// One export: what to render and how to write it.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    /// The file to write.
    pub path: PathBuf,
    /// Its type.
    pub file_type: FileType,
    /// Its bits per sample.
    pub bit_depth: BitDepth,
    /// The dither below 32 bits.
    pub dither: Dither,
    /// The file's sample rate.
    pub sample_rate: u32,
    /// The engine's own rate: a file rate below it is rendered at it and
    /// converted down.
    pub engine_rate: u32,
    /// Bring the peak to full scale.
    pub normalize: bool,
    /// Where the range starts.
    pub start: Ticks,
    /// How long it is.
    pub length: Length,
    /// Replace a file already at `path`.
    pub replace: bool,
    /// The engine's block size, from 1 to 8,192 frames.
    pub block: usize,
}

/// What an export did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Frames in the file.
    pub frames: u64,
    /// Channels in each frame.
    pub channels: u16,
    /// The rate the engine rendered at.
    pub render_rate: u32,
    /// Frames the engine rendered.
    pub rendered_frames: u64,
    /// The largest sample magnitude written, before rounding.
    pub peak: f32,
    /// The gain Normalize applied: 1 without it.
    pub gain: f64,
    /// How long the export took.
    pub took: Duration,
    /// Media that clips name and the media library couldn't supply: those
    /// clips played silence.
    pub missing_media: Vec<ContentHash>,
    /// Devices no factory makes a node for, whose tracks' audio passed
    /// through where they'd be.
    pub unhosted: Vec<DeviceId>,
    /// Heap operations in the engine's blocks, where the allocation trap is
    /// installed. The real-time rules allow none.
    pub heap_operations: u64,
}

impl Report {
    /// How many times faster than real time the export ran: the file's
    /// length divided by the time it took.
    #[must_use]
    pub fn speed(&self, sample_rate: u32) -> f64 {
        let seconds = self.frames as f64 / f64::from(sample_rate.max(1));
        seconds / self.took.as_secs_f64().max(1e-9)
    }
}

/// Why an export didn't happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportError {
    /// It was cancelled. Nothing was left behind.
    Cancelled,
    /// A setting can't be exported, for the reason given. Nothing was
    /// written.
    Invalid(String),
    /// A file is at the path already, and the job doesn't replace it.
    Exists(PathBuf),
    /// Rendering or writing failed. Nothing was left behind.
    Failed(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("the export was cancelled"),
            Self::Invalid(why) => f.write_str(why),
            Self::Exists(path) => write!(
                f,
                "{} exists already; export to another file, or replace it",
                path.display()
            ),
            Self::Failed(why) => write!(f, "the export failed: {why}"),
        }
    }
}

impl std::error::Error for ExportError {}

/// Where the song ends: the end of its last clip, if it has clips.
#[must_use]
pub fn song_end(document: &Document) -> Option<Ticks> {
    document
        .tracks()
        .flat_map(bl_model::Track::clips)
        .map(bl_model::Clip::end)
        .max()
}

/// The range to export (the export spec, R1): `start` and `length` where
/// given, otherwise the Arrangement's time selection, otherwise the whole
/// set, from 1.1.1 to the end of its last clip. Returns the start, the end,
/// and where they came from.
///
/// # Errors
///
/// A message if there's nothing to export (no clips and no selection), the
/// range starts before 1.1.1, or it's empty.
pub fn range(
    document: &Document,
    start: Option<&Position>,
    length: Option<Ticks>,
    selection: Option<(Ticks, Ticks)>,
) -> Result<(Ticks, Ticks, RangeSource), String> {
    let signatures: &TimeSignatureMap = &document.settings().time_signatures;
    let start = start.map(|start| start.resolve(signatures)).transpose()?;
    let whole = || {
        song_end(document)
            .map(|end| (Ticks::ZERO, end))
            .ok_or_else(|| {
                "the set has no clips, so there's nothing to export; make a time selection or \
             give a start and a length"
                    .to_owned()
            })
    };
    let (from, to, source) = match (start, length, selection) {
        (None, None, Some((from, to))) => (from, to, RangeSource::Selection),
        (None, None, None) => {
            let (from, to) = whole()?;
            (from, to, RangeSource::Set)
        }
        (Some(from), Some(length), _) => (from, from + length, RangeSource::Arguments),
        (Some(from), None, selection) => {
            let to = match selection {
                Some((_, to)) => to,
                None => whole()?.1,
            };
            (from, to, RangeSource::Arguments)
        }
        (None, Some(length), selection) => {
            let from = selection.map_or(Ticks::ZERO, |(from, _)| from);
            (from, from + length, RangeSource::Arguments)
        }
    };
    if from < Ticks::ZERO {
        return Err("the range starts before 1.1.1".to_owned());
    }
    if to <= from {
        return Err("the range is empty: it must end after it starts".to_owned());
    }
    Ok((from, to, source))
}

/// `.<name>.<kind>` beside `path`: a hidden file in the same folder, so
/// moving it into place is a rename on one disk.
#[must_use]
pub fn hidden_beside(path: &Path, kind: &str) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(".");
    name.push(kind);
    path.with_file_name(name)
}

/// Runs an export of `document`, with its clips' audio from `media`. It
/// checks `cancel` between blocks and calls `progress` with the share done,
/// from 0 to 1.
///
/// # Errors
///
/// [`ExportError`]: cancelled, a setting that can't be exported, an
/// existing file, or a render or write that failed. The destination is as
/// it was before, and nothing else is left behind.
pub fn export(
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    job: &Job,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Report, ExportError> {
    check(job)?;
    let partial = hidden_beside(&job.path, "partial");
    let scratch = hidden_beside(&job.path, "render");
    let result = run(document, media, job, &partial, &scratch, cancel, progress);
    // The scratch file goes either way; the partial file once it's moved or
    // if anything failed.
    let _ = fs::remove_file(&scratch);
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

/// Checks a job before anything is rendered or written.
fn check(job: &Job) -> Result<(), ExportError> {
    let invalid = |why: String| Err(ExportError::Invalid(why));
    if !RATES.contains(&job.sample_rate) {
        return invalid(format!(
            "a sample rate of {} Hz; exports take {} to {} Hz",
            job.sample_rate,
            RATES.start(),
            RATES.end()
        ));
    }
    if !RATES.contains(&job.engine_rate) {
        return invalid(format!("an engine rate of {} Hz", job.engine_rate));
    }
    if !(1..=8192).contains(&job.block) {
        return invalid(format!("a block of {} frames", job.block));
    }
    if !job.file_type.holds(job.bit_depth) {
        return invalid("FLAC holds 16 or 24 bits; export 32-bit float as WAV or AIFF".to_owned());
    }
    if job.path.file_name().is_none() || job.path.is_dir() {
        return invalid(format!("{} isn't a file name", job.path.display()));
    }
    let folder = match job.path.parent() {
        Some(folder) if !folder.as_os_str().is_empty() => folder,
        _ => Path::new("."),
    };
    if !folder.is_dir() {
        return invalid(format!("there's no folder {}", folder.display()));
    }
    if !job.replace && job.path.exists() {
        return Err(ExportError::Exists(job.path.clone()));
    }
    Ok(())
}

/// What to render: a range of the song, at a rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Render {
    /// Where the range starts.
    pub start: Ticks,
    /// How long it is.
    pub length: Length,
    /// The rate the frames come out at.
    pub sample_rate: u32,
    /// The engine's own rate: a lower `sample_rate` is rendered at it and
    /// converted down.
    pub engine_rate: u32,
    /// The engine's block size, from 1 to 8,192 frames.
    pub block: usize,
}

/// What a render did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rendered {
    /// Frames handed on, at the render's `sample_rate`.
    pub frames: u64,
    /// Channels in each frame.
    pub channels: u16,
    /// The rate the engine rendered at.
    pub render_rate: u32,
    /// Frames the engine rendered.
    pub rendered_frames: u64,
    /// Media that clips name and the media library couldn't supply.
    pub missing_media: Vec<ContentHash>,
    /// Devices no factory makes a node for.
    pub unhosted: Vec<DeviceId>,
    /// Heap operations in the engine's blocks.
    pub heap_operations: u64,
}

/// Renders a range of `document` offline, with its clips' audio from
/// `media`, through the engine code that plays the set on a device
/// ([`Offline`]): the song plays from the range's start, as Play does from
/// an insert marker there, with the Arrangement loop off, as fast as the
/// computer allows, on the calling thread. It hands `sink` the frames in
/// order, interleaved, at the render's `sample_rate` (converted down with
/// the media converter where that's below the engine's). Exports and the
/// agent API's previews (WP-1.16) both render through it. It checks
/// `cancel` between blocks and calls `progress` with the share rendered,
/// from 0 to 1. `longest` is the most seconds it renders.
///
/// # Errors
///
/// [`ExportError`]: cancelled, a range it can't render, a sink that
/// failed, or samples that aren't finite numbers.
pub fn render(
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    spec: &Render,
    longest: f64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
    sink: &mut dyn FnMut(&[f32]) -> Result<(), ExportError>,
) -> Result<Rendered, ExportError> {
    if !RATES.contains(&spec.sample_rate) {
        return Err(ExportError::Invalid(format!(
            "a sample rate of {} Hz; renders take {} to {} Hz",
            spec.sample_rate,
            RATES.start(),
            RATES.end()
        )));
    }
    if !RATES.contains(&spec.engine_rate) {
        return Err(ExportError::Invalid(format!(
            "an engine rate of {} Hz",
            spec.engine_rate
        )));
    }
    if !(1..=8192).contains(&spec.block) {
        return Err(ExportError::Invalid(format!(
            "a block of {} frames",
            spec.block
        )));
    }
    // Spec: export R6. A rate at or above the engine's renders at it; a
    // lower one renders at the engine's and is converted down.
    let render_hz = spec.sample_rate.max(spec.engine_rate);
    let render_rate = SampleRate::new(render_hz)
        .ok_or_else(|| ExportError::Invalid(format!("a rate of {render_hz} Hz")))?;
    let tempo = &document.settings().tempo;
    let first = tempo.ticks_to_frames(spec.start, render_rate).get();
    let frames = match spec.length {
        Length::Frames(frames) => frames,
        Length::To(end) => {
            u64::try_from(tempo.ticks_to_frames(end, render_rate).get() - first).unwrap_or(0)
        }
    };
    if frames == 0 {
        return Err(ExportError::Invalid(
            "the range is shorter than one frame".to_owned(),
        ));
    }
    if frames as f64 > longest * f64::from(render_hz) {
        return Err(ExportError::Invalid(format!(
            "the range is longer than the longest render, {longest} seconds"
        )));
    }

    let config = EngineConfig::new(render_rate, spec.block);
    let channels = config.channels;
    let mut offline = Offline::new(config, Registry::new().with_media(media));
    let unhosted = offline.sync(document);
    let missing_media = offline.missing_media().to_vec();
    // Spec: export R3. The range plays straight through from its start,
    // as Play from an insert marker there does, without the loop.
    offline.sender().transport(TransportCommand::SetLoop(None));
    offline
        .sender()
        .transport(TransportCommand::Locate(spec.start));
    offline
        .sender()
        .transport_at(TransportCommand::Play, When::Frame(0));

    let mut converter = (spec.sample_rate < render_hz)
        .then(|| Downsampler::new(render_hz, spec.sample_rate, channels, frames))
        .transpose()?;
    let mut non_finite = 0u64;
    let mut out_frames = 0u64;
    let mut forward = |samples: &[f32]| -> Result<(), ExportError> {
        non_finite += samples.iter().filter(|s| !s.is_finite()).count() as u64;
        out_frames += (samples.len() / channels) as u64;
        sink(samples)
    };
    let mut samples = Vec::with_capacity(spec.block * channels);
    let mut done = 0u64;
    while done < frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        let block = (spec.block as u64).min(frames - done) as usize;
        let output = offline.process(block);
        interleave(&output, &mut samples);
        match converter.as_mut() {
            Some(converter) => converter.push(&samples, &mut forward)?,
            None => forward(&samples)?,
        }
        done += block as u64;
        progress(done as f64 / frames as f64);
    }
    if let Some(converter) = converter.as_mut() {
        converter.finish(&mut forward)?;
    }
    if non_finite > 0 {
        return Err(ExportError::Failed(format!(
            "{non_finite} samples of the render aren't finite numbers"
        )));
    }
    Ok(Rendered {
        frames: out_frames,
        channels: channels as u16,
        render_rate: render_hz,
        rendered_frames: frames,
        missing_media,
        unhosted,
        heap_operations: offline.heap_operations(),
    })
}

/// Where an export's render goes: straight to the writer, or, with
/// Normalize, to the scratch file for a second pass.
enum Out {
    Writer(AudioWriter),
    Scratch(BufWriter<File>, Vec<u8>),
}

fn interleave(output: &Output<'_>, samples: &mut Vec<f32>) {
    samples.clear();
    for frame in 0..output.frames() {
        samples.extend((0..output.channels()).map(|c| output.channel(c)[frame]));
    }
}

fn run(
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    job: &Job,
    partial: &Path,
    scratch: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Report, ExportError> {
    let began = Instant::now();
    let failed = |what: &Path, error: &dyn std::fmt::Display| {
        ExportError::Failed(format!("{}: {error}", what.display()))
    };
    let spec = EncodeSpec {
        file_type: job.file_type,
        bit_depth: job.bit_depth,
        dither: if job.bit_depth.is_integer() {
            job.dither
        } else {
            Dither::None
        },
        sample_rate: job.sample_rate,
        // The channels the engine renders, which `render` makes the same
        // way.
        channels: EngineConfig::new(
            SampleRate::new(job.sample_rate.max(job.engine_rate))
                .ok_or_else(|| ExportError::Invalid(format!("a rate of {} Hz", job.engine_rate)))?,
            job.block,
        )
        .channels as u16,
    };
    let mut out = if job.normalize {
        let file = File::create(scratch).map_err(|error| failed(scratch, &error))?;
        Out::Scratch(BufWriter::with_capacity(1 << 16, file), Vec::new())
    } else {
        Out::Writer(AudioWriter::create(partial, spec).map_err(|error| failed(partial, &error))?)
    };

    // Pass one: the render.
    let passes = if job.normalize { 2.0 } else { 1.0 };
    let mut peak = 0.0f32;
    let rendered = render(
        document,
        media,
        &Render {
            start: job.start,
            length: job.length,
            sample_rate: job.sample_rate,
            engine_rate: job.engine_rate,
            block: job.block,
        },
        MAX_SECONDS,
        cancel,
        &mut |share| progress(share / passes),
        &mut |samples| {
            for &sample in samples {
                if sample.is_finite() {
                    peak = peak.max(sample.abs());
                }
            }
            match &mut out {
                Out::Writer(writer) => writer
                    .write(samples)
                    .map_err(|error| ExportError::Failed(error.to_string())),
                Out::Scratch(file, bytes) => {
                    bytes.clear();
                    for sample in samples {
                        bytes.extend_from_slice(&sample.to_le_bytes());
                    }
                    file.write_all(bytes)
                        .map_err(|error| ExportError::Failed(format!("the scratch file: {error}")))
                }
            }
        },
    )?;
    let channels = usize::from(rendered.channels);

    // Pass two, with Normalize: the scratch file through the gain to the
    // writer. Spec: export R8.
    let mut gain = 1.0;
    let writer = match out {
        Out::Writer(writer) => writer,
        Out::Scratch(file, _) => {
            file.into_inner()
                .map_err(|error| failed(scratch, &error.error().to_string()))?;
            if peak > 0.0 {
                gain = 1.0 / f64::from(peak);
            }
            let mut writer =
                AudioWriter::create(partial, spec).map_err(|error| failed(partial, &error))?;
            let mut reader = BufReader::with_capacity(
                1 << 16,
                File::open(scratch).map_err(|error| failed(scratch, &error))?,
            );
            let chunk_frames = 8192;
            let mut bytes = vec![0u8; chunk_frames * channels * 4];
            let mut chunk = Vec::with_capacity(chunk_frames * channels);
            let total = rendered.frames;
            let mut read = 0u64;
            peak = 0.0;
            while read < total {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ExportError::Cancelled);
                }
                let take = (chunk_frames as u64).min(total - read) as usize;
                let bytes = &mut bytes[..take * channels * 4];
                reader
                    .read_exact(bytes)
                    .map_err(|error| failed(scratch, &error))?;
                chunk.clear();
                chunk.extend(
                    bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| (f64::from(f32::from_le_bytes(*b)) * gain) as f32),
                );
                for sample in &chunk {
                    peak = peak.max(sample.abs());
                }
                writer
                    .write(&chunk)
                    .map_err(|error| failed(partial, &error))?;
                read += take as u64;
                progress(0.5 + read as f64 / total as f64 / 2.0);
            }
            writer
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(ExportError::Cancelled);
    }
    let written = writer.finish().map_err(|error| failed(partial, &error))?;
    // Spec: export R10. Only a complete file reaches the destination.
    fs::rename(partial, &job.path).map_err(|error| failed(&job.path, &error))?;
    progress(1.0);
    Ok(Report {
        frames: written,
        channels: rendered.channels,
        render_rate: rendered.render_rate,
        rendered_frames: rendered.rendered_frames,
        peak,
        gain,
        took: began.elapsed(),
        missing_media: rendered.missing_media,
        unhosted: rendered.unhosted,
        heap_operations: rendered.heap_operations,
    })
}

/// Runs `export.audio` with its arguments: the set in `document`, its
/// clips' audio from `media`, the Arrangement's time `selection`, and the
/// engine's rate.
///
/// # Errors
///
/// [`ShellError`] for arguments that can't be exported, an existing file,
/// or an export that failed.
pub fn run_command(
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    selection: Option<(Ticks, Ticks)>,
    engine_rate: u32,
    args: &ExportArgs,
) -> Result<ExportResult, ShellError> {
    let invalid = |message: String| ShellError::InvalidArgs {
        command: AUDIO.to_owned(),
        message,
    };
    let path = PathBuf::from(&args.path);
    let named = FileType::from_path(&path);
    let file_type = match (args.file_type, named) {
        (None, Some(named)) => named,
        (None, None) => {
            return Err(invalid(format!(
                "{} has no audio file extension; end it in .wav, .aif, or .flac, or give \
                 `file_type`",
                args.path
            )));
        }
        (Some(asked), named) => {
            let asked = match asked {
                SpecFileType::Wav => FileType::Wav,
                SpecFileType::Aiff => FileType::Aiff,
                SpecFileType::Flac => FileType::Flac,
            };
            if named.is_some_and(|named| named != asked) {
                return Err(invalid(format!(
                    "`file_type` is {} but {} ends in another type's extension",
                    args.file_type.map_or("", |t| t.name()),
                    args.path
                )));
            }
            asked
        }
    };
    let bit_depth = BitDepth::from_bits(u32::from(args.bit_depth))
        .ok_or_else(|| invalid(format!("{} bits; give 16, 24, or 32", args.bit_depth)))?;
    let dither = match args.dither.unwrap_or_default() {
        SpecDither::None => Dither::None,
        SpecDither::Rectangular => Dither::Rectangular,
        SpecDither::Triangular => Dither::Triangular,
    };
    let (start, end, source) =
        range(document, args.start.as_ref(), args.length, selection).map_err(invalid)?;
    let job = Job {
        path: path.clone(),
        file_type,
        bit_depth,
        dither,
        sample_rate: args.sample_rate,
        engine_rate,
        normalize: args.normalize,
        start,
        length: Length::To(end),
        replace: args.replace,
        block: BLOCK,
    };
    let report =
        export(document, media, &job, &AtomicBool::new(false), &mut |_| {}).map_err(|error| {
            match error {
                ExportError::Invalid(message) => invalid(message),
                ExportError::Exists(_) => ShellError::Unavailable {
                    command: AUDIO.to_owned(),
                    reason: format!("{error}: set `replace` to replace it"),
                },
                ExportError::Cancelled | ExportError::Failed(_) => ShellError::Failed {
                    command: AUDIO.to_owned(),
                    message: error.to_string(),
                },
            }
        })?;
    let written_path = fs::canonicalize(&path).unwrap_or(path);
    Ok(ExportResult {
        path: written_path.display().to_string(),
        file_type: match file_type {
            FileType::Wav => SpecFileType::Wav,
            FileType::Aiff => SpecFileType::Aiff,
            FileType::Flac => SpecFileType::Flac,
        },
        bit_depth: args.bit_depth,
        sample_rate: args.sample_rate,
        channels: report.channels,
        frames: report.frames,
        seconds: report.frames as f64 / f64::from(args.sample_rate),
        start,
        end,
        range: source,
        dither: if bit_depth.is_integer() {
            args.dither.unwrap_or_default()
        } else {
            SpecDither::None
        },
        peak_dbfs: (report.peak > 0.0).then(|| 20.0 * f64::from(report.peak).log10()),
        gain_db: 20.0 * report.gain.log10(),
        render_rate: report.render_rate,
        took_seconds: report.took.as_secs_f64(),
        speed: report.speed(args.sample_rate),
        missing_media: report
            .missing_media
            .iter()
            .map(ToString::to_string)
            .collect(),
    })
}

/// Converts a render down to the file's rate as it arrives, a chunk at a
/// time, exactly as [`RateConverter::convert`] converts it whole: the
/// converter's delay dropped, and `ceil(frames × to ÷ from)` frames out.
struct Downsampler {
    converter: RateConverter,
    channels: usize,
    /// Input waiting for a whole chunk, one buffer per channel.
    pending: Vec<Vec<f32>>,
    /// The converter's output, one buffer per channel.
    out: Vec<Vec<f32>>,
    interleaved: Vec<f32>,
    skip: usize,
    expected: u64,
    emitted: u64,
}

impl Downsampler {
    fn new(from: u32, to: u32, channels: usize, frames: u64) -> Result<Self, ExportError> {
        let converter = RateConverter::new(from, to, channels)
            .map_err(|error| ExportError::Failed(error.to_string()))?;
        let room = converter.output_frames_max();
        Ok(Self {
            skip: converter.output_delay(),
            expected: bl_media::converted_len(frames as usize, from, to) as u64,
            converter,
            channels,
            pending: vec![Vec::new(); channels],
            out: vec![vec![0.0; room]; channels],
            interleaved: Vec::new(),
            emitted: 0,
        })
    }

    fn push(
        &mut self,
        samples: &[f32],
        sink: &mut dyn FnMut(&[f32]) -> Result<(), ExportError>,
    ) -> Result<(), ExportError> {
        for frame in samples.chunks_exact(self.channels) {
            for (pending, &sample) in self.pending.iter_mut().zip(frame) {
                pending.push(sample);
            }
        }
        while self.pending[0].len() >= self.converter.input_frames_next() {
            self.step(sink)?;
        }
        Ok(())
    }

    fn finish(
        &mut self,
        sink: &mut dyn FnMut(&[f32]) -> Result<(), ExportError>,
    ) -> Result<(), ExportError> {
        // The converter pads the end with silence; a few chunks flush it.
        let mut steps = 0;
        while self.emitted < self.expected {
            self.step(sink)?;
            steps += 1;
            if steps > 64 {
                return Err(ExportError::Failed(
                    "the sample-rate converter stopped giving output".to_owned(),
                ));
            }
        }
        Ok(())
    }

    fn step(
        &mut self,
        sink: &mut dyn FnMut(&[f32]) -> Result<(), ExportError>,
    ) -> Result<(), ExportError> {
        let take = self
            .converter
            .input_frames_next()
            .min(self.pending[0].len());
        let input: Vec<&[f32]> = self.pending.iter().map(|c| &c[..take]).collect();
        let mut output: Vec<&mut [f32]> = self.out.iter_mut().map(|c| &mut c[..]).collect();
        let (used, written) = self
            .converter
            .process(&input, &mut output)
            .map_err(|error| ExportError::Failed(error.to_string()))?;
        for pending in &mut self.pending {
            pending.drain(..used);
        }
        let dropped = self.skip.min(written);
        self.skip -= dropped;
        let keep = ((written - dropped) as u64).min(self.expected - self.emitted) as usize;
        self.interleaved.clear();
        for frame in dropped..dropped + keep {
            self.interleaved
                .extend(self.out.iter().map(|channel| channel[frame]));
        }
        self.emitted += keep as u64;
        if keep > 0 {
            sink(&self.interleaved)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_files_sit_beside_the_destination() {
        assert_eq!(
            hidden_beside(Path::new("out/Mix.wav"), "partial"),
            Path::new("out/.Mix.wav.partial")
        );
        assert_eq!(
            hidden_beside(Path::new("Mix.flac"), "render"),
            Path::new(".Mix.flac.render")
        );
    }

    #[test]
    fn streaming_conversion_matches_whole_conversion() {
        // Spec: export R6. Converting as the render arrives, in blocks of
        // any size, gives what converting it whole does.
        let frames = 30_011;
        let left: Vec<f32> = (0..frames)
            .map(|n| (n as f32 * 0.013).sin() * 0.7)
            .collect();
        let right: Vec<f32> = (0..frames)
            .map(|n| (n as f32 * 0.0021).cos() * 0.4)
            .collect();
        let whole = bl_media::convert_sample_rate(
            &bl_media::SampleData::new(48_000, vec![left.clone(), right.clone()]).unwrap(),
            44_100,
        )
        .unwrap();
        for block in [1, 100, 512, 4096, 9000] {
            let mut converter = Downsampler::new(48_000, 44_100, 2, frames as u64).unwrap();
            let mut out = Vec::new();
            let mut sink = |samples: &[f32]| {
                out.extend_from_slice(samples);
                Ok(())
            };
            let interleaved: Vec<f32> = left
                .iter()
                .zip(&right)
                .flat_map(|(&l, &r)| [l, r])
                .collect();
            for chunk in interleaved.chunks(block * 2) {
                converter.push(chunk, &mut sink).unwrap();
            }
            converter.finish(&mut sink).unwrap();
            assert_eq!(out.len(), whole.frames() * 2, "block {block}");
            for (n, frame) in out.chunks(2).enumerate() {
                assert_eq!(frame[0].to_bits(), whole.channel(0).unwrap()[n].to_bits());
                assert_eq!(frame[1].to_bits(), whole.channel(1).unwrap()[n].to_bits());
            }
        }
    }
}
