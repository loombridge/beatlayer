//! Exporting audio (WP-1.12; MP3, WP-2.26): a range of the set rendered
//! offline through the engine and written to a WAV, AIFF, FLAC, or MP3
//! file. The behaviour it follows is in the export spec.
//!
//! [`export`] runs one export on the calling thread:
//!
//! 1. The engine renders the range with [`bl_engine::Offline`], the code
//!    behind `bl-cli render` and the golden tests, which runs the same
//!    processor a device's audio thread runs, block after block, as fast as
//!    it can. The song plays from the range's start, as Play does from an
//!    insert marker there, with the Arrangement loop off. Plug-in devices
//!    render with the plug-ins the engine plays, lent for the render
//!    ([`Hosts::lend`], WP-2.6b, WP-4.21).
//! 2. At a file rate below the engine's, the render is converted down with
//!    the media converter; otherwise the engine renders at the file's rate.
//! 3. With Normalize, or Render as Loop, the render goes to a scratch file
//!    first, so the gain can be worked out from the peak and applied on a
//!    second pass. Render as Loop also renders the song's tail past the
//!    range's end and adds it onto the file's start before that pass.
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
use bl_media::{AudioWriter, BitDepth, Dither, EncodeSpec, FileType, MP3_RATES, RateConverter};
use bl_model::{ContentHash, DeviceId, Document, TrackId};
pub use bl_plugin_host::Hosts;
use bl_shell_spec::editing::Position;
use bl_shell_spec::export::{
    AUDIO, Dither as SpecDither, Dropout as SpecDropout, ExportArgs, ExportResult,
    FileType as SpecFileType, RangeSource, RenderedTrack,
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

/// Spec: export R25. A Render as Loop tail ends once the output has stayed
/// below −120 dBFS for [`TAIL_QUIET`] seconds, or after [`TAIL_SECONDS`].
const TAIL_SECONDS: u64 = 60;
const TAIL_QUIET: f64 = 0.1;
const TAIL_FLOOR: f32 = 1e-6;

/// Where a render hands its frames: interleaved, in order.
pub type Sink<'a> = dyn FnMut(&[f32]) -> Result<(), ExportError> + 'a;

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
    /// Add the tail past the range's end onto the file's start (Render as
    /// Loop, the export spec, R24).
    pub render_as_loop: bool,
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

/// Runs an export of `document`, with its clips' audio from `media` and its
/// plug-in devices' plug-ins borrowed from `plugins` (a format without a
/// host passes its input through). It checks `cancel` between blocks and
/// calls `progress` with the share done, from 0 to 1.
///
/// # Errors
///
/// [`ExportError`]: cancelled, a setting that can't be exported, an
/// existing file, or a render or write that failed. The destination is as
/// it was before, and nothing else is left behind.
pub fn export(
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    plugins: Hosts<'_>,
    job: &Job,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Report, ExportError> {
    let mut reports = export_multi(&[(document, job)], media, plugins, cancel, progress)?;
    Ok(reports.pop().unwrap())
}

/// Runs multiple exports as a batch (WP-2.26): checks all jobs first,
/// renders each to a hidden partial file, and promotes them to their final
/// destinations once all renders succeed. Cancelling or any render failure
/// deletes all partial and scratch files and leaves every destination untouched.
/// If moving a completed partial file into place fails, any earlier destinations
/// already promoted remain in place while the remaining partial files are deleted.
///
/// # Errors
///
/// [`ExportError`]: cancelled, a setting that can't be exported, an
/// existing file, or a render or write that failed. The destinations stay as
/// they were before (or files already moved on rename failure), and no partial
/// files are left behind.
pub fn export_multi(
    renders: &[(&Document, &Job)],
    media: Arc<dyn MediaLibrary>,
    plugins: Hosts<'_>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Vec<Report>, ExportError> {
    let (reports, partials) = render_multi(renders, media, plugins, cancel, progress)?;
    let destinations: Vec<PathBuf> = renders.iter().map(|(_, job)| job.path.clone()).collect();
    promote(&partials, &destinations)?;
    progress(1.0);
    Ok(reports)
}

/// Moves the finished files into place, one by one. If a rename fails, the
/// files already moved remain in place while the rest are deleted.
fn promote(partials: &[PathBuf], destinations: &[PathBuf]) -> Result<(), ExportError> {
    for (partial, destination) in partials.iter().zip(destinations) {
        if let Err(error) = fs::rename(partial, destination) {
            for p in partials {
                let _ = fs::remove_file(p);
            }
            return Err(ExportError::Failed(format!(
                "{}: {error}",
                destination.display()
            )));
        }
    }
    Ok(())
}

/// Renders the exports to the hidden files beside their destinations,
/// without moving them into place: the caller may render more before the
/// files are complete (WP-5.35). Returns the reports and the partial files.
pub fn render_multi(
    renders: &[(&Document, &Job)],
    media: Arc<dyn MediaLibrary>,
    plugins: Hosts<'_>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<(Vec<Report>, Vec<PathBuf>), ExportError> {
    if renders.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    for (_, job) in renders {
        check(job)?;
    }

    let mut partials: Vec<PathBuf> = Vec::with_capacity(renders.len());
    let mut scratches: Vec<PathBuf> = Vec::with_capacity(renders.len());
    for (_, job) in renders {
        partials.push(hidden_beside(&job.path, "partial"));
        scratches.push(hidden_beside(&job.path, "render"));
    }

    let total = renders.len() as f64;
    let mut reports = Vec::with_capacity(renders.len());
    let mut render_result = Ok(());

    for (i, ((document, job), partial)) in renders.iter().zip(&partials).enumerate() {
        let scratch = &scratches[i];
        let files = (partial.as_path(), scratch.as_path());
        let mut job_progress = |fraction: f64| {
            let overall = (i as f64 + fraction) / total;
            progress(overall);
        };
        match run(
            document,
            media.clone(),
            plugins,
            job,
            files,
            cancel,
            &mut job_progress,
        ) {
            Ok(report) => reports.push(report),
            Err(error) => {
                render_result = Err(error);
                break;
            }
        }
        let _ = fs::remove_file(scratch);
    }

    for scratch in &scratches {
        let _ = fs::remove_file(scratch);
    }

    if let Err(error) = render_result {
        for partial in &partials {
            let _ = fs::remove_file(partial);
        }
        return Err(error);
    }

    Ok((reports, partials))
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
    // Spec: export R23 (WP-2.26). MP3 holds its own rates; anything else is
    // refused before the render. Its bit depth and dither don't apply.
    if job.file_type == FileType::Mp3 && !MP3_RATES.contains(&job.sample_rate) {
        let rates: Vec<String> = MP3_RATES.iter().map(u32::to_string).collect();
        return invalid(format!(
            "a sample rate of {} Hz; MP3 holds {} Hz",
            job.sample_rate,
            rates.join(", ")
        ));
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
    /// Frames of the range handed on, at the render's `sample_rate`. A
    /// tail goes to its own sink and isn't counted.
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
/// `media` and its plug-in devices' plug-ins borrowed from `plugins` (the
/// offline plug-in renders spec; a set without a plug-in device borrows
/// nothing), through the engine code that plays the set on a device
/// ([`Offline`]): the song plays from the range's start, as Play does from
/// an insert marker there, with the Arrangement loop off, as fast as the
/// computer allows, on the calling thread. It hands `sink` the frames in
/// order, interleaved, at the render's `sample_rate` (converted down with
/// the media converter where that's below the engine's). Exports and the
/// agent API's previews (WP-1.16) both render through it. It checks
/// `cancel` between blocks and calls `progress` with the share rendered,
/// from 0 to 1. `longest` is the most seconds it renders.
///
/// With a `tail` sink, the song goes on past the range's end as if Stop were
/// pressed there, until the output has stayed below −120 dBFS for 100 ms or
/// 60 s have passed (the export spec, R25): `sink` gets only the range's
/// frames, and `tail` the frames after them.
///
/// # Errors
///
/// [`ExportError`]: cancelled, a range it can't render, a plug-in it
/// couldn't borrow, a sink that failed, or samples that aren't finite
/// numbers. Whatever the outcome, every plug-in goes back to the engine.
#[expect(clippy::too_many_arguments, reason = "the render's inputs")]
pub fn render(
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    plugins: Hosts<'_>,
    spec: &Render,
    longest: f64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
    sink: &mut Sink<'_>,
    mut tail: Option<&mut Sink<'_>>,
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
    // The range is measured through the tempo map the set plays: the leading
    // clips' tempo included (the warping spec, R48).
    let tempo = &document.tempo_map();
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
    // Spec: offline-plugin-renders R1. The plug-ins the engine plays, lent
    // to the render. The loan is made before the render's engine, so the
    // engine, and its nodes holding the plug-ins, go first, however the
    // render ends, and the loan then gives every plug-in back (R4). A set
    // without a plug-in device borrows nothing: its render never touches
    // the hosts or the engine playing.
    let loan = plugins
        .lend(|format| crate::plugins::has_format(document, format))
        .map_err(ExportError::Failed)?;
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    let kit = bl_devices::kit_host();
    registry.add(kit.factory());
    loan.register(&mut registry);
    kit.sync(document);
    let mut offline = Offline::new(config, registry.with_media(media));
    let unhosted = offline.sync(document);
    kit.idle();
    // Spec: offline-plugin-renders R2. A plug-in the engine didn't hand
    // back would render as a pass-through, which isn't what plays.
    if let Some((device, why)) = loan.problems().pop() {
        return Err(ExportError::Failed(format!(
            "the plug-in on {device} couldn't be rendered: {why}"
        )));
    }
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
    // Spec: export R25. The tail starts with the range's last frame done.
    let with_tail = tail.is_some();
    if with_tail {
        offline
            .sender()
            .transport_at(TransportCommand::Stop, When::Frame(frames));
    }

    let mut converter = (spec.sample_rate < render_hz)
        .then(|| Downsampler::new(render_hz, spec.sample_rate, channels, frames))
        .transpose()?;
    let mut non_finite = 0u64;
    let mut out_frames = 0u64;
    // The frames of the range at the file's rate; what follows is the tail.
    let range_out = if with_tail {
        bl_media::converted_len(frames as usize, render_hz, spec.sample_rate) as u64
    } else {
        u64::MAX
    };
    let mut forward = |samples: &[f32]| -> Result<(), ExportError> {
        non_finite += samples.iter().filter(|s| !s.is_finite()).count() as u64;
        let in_range = ((samples.len() / channels) as u64).min(range_out - out_frames);
        out_frames += in_range;
        let (range, after) = samples.split_at(in_range as usize * channels);
        if !range.is_empty() {
            sink(range)?;
        }
        match tail.as_mut() {
            Some(tail) if !after.is_empty() => tail(after),
            _ => Ok(()),
        }
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
    if with_tail {
        let mut tail_frames = 0u64;
        let longest_tail = TAIL_SECONDS * u64::from(render_hz);
        let quiet_for = (TAIL_QUIET * f64::from(render_hz)) as u64;
        let mut quiet = 0u64;
        if let Some(converter) = converter.as_mut() {
            converter.expected = u64::MAX;
        }
        while quiet < quiet_for && tail_frames < longest_tail {
            if cancel.load(Ordering::Relaxed) {
                return Err(ExportError::Cancelled);
            }
            let block = (spec.block as u64).min(longest_tail - tail_frames) as usize;
            interleave(&offline.process(block), &mut samples);
            // The tail ends on the frame that completes the quiet stretch,
            // whatever the block size.
            let mut keep = block;
            for (n, frame) in samples.chunks_exact(channels).enumerate() {
                quiet = if frame.iter().all(|s| s.abs() < TAIL_FLOOR) {
                    quiet + 1
                } else {
                    0
                };
                if quiet >= quiet_for {
                    keep = n + 1;
                    break;
                }
            }
            samples.truncate(keep * channels);
            match converter.as_mut() {
                Some(converter) => converter.push(&samples, &mut forward)?,
                None => forward(&samples)?,
            }
            tail_frames += keep as u64;
        }
        if let Some(converter) = converter.as_mut() {
            converter.expected = bl_media::converted_len(
                (frames + tail_frames) as usize,
                render_hz,
                spec.sample_rate,
            ) as u64;
        }
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
fn interleave(output: &Output<'_>, samples: &mut Vec<f32>) {
    samples.clear();
    for frame in 0..output.frames() {
        samples.extend((0..output.channels()).map(|c| output.channel(c)[frame]));
    }
}

pub(crate) enum Out {
    /// The file itself: the writer that encodes and rounds.
    Writer(AudioWriter),
    /// The scratch file the normalise pass reads: the raw samples.
    Scratch(BufWriter<File>, Vec<u8>),
}

/// How a file is encoded: its type, its bits, its dither, its rate, and
/// its channels.
pub(crate) fn encode_spec(job: &Job) -> Result<EncodeSpec, ExportError> {
    Ok(EncodeSpec {
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
    })
}

/// Opens the file a job writes: the writer for the finished file, or the
/// scratch file the normalise pass reads.
pub(crate) fn open(
    job: &Job,
    (partial, scratch): (&Path, &Path),
    spec: EncodeSpec,
) -> Result<Out, ExportError> {
    let failed = |what: &Path, error: &dyn std::fmt::Display| {
        ExportError::Failed(format!("{}: {error}", what.display()))
    };
    if job.normalize {
        let file = File::create(scratch).map_err(|error| failed(scratch, &error))?;
        Ok(Out::Scratch(BufWriter::with_capacity(1 << 16, file), Vec::new()))
    } else {
        let writer =
            AudioWriter::create(partial, spec).map_err(|error| failed(partial, &error))?;
        Ok(Out::Writer(writer))
    }
}

/// Writes samples into the file: the writer, or the scratch file.
pub(crate) fn write(out: &mut Out, samples: &[f32]) -> Result<(), ExportError> {
    match out {
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
}

/// Finishes a render's file: with Normalize, the scratch file through the
/// gain to a new writer for the finished file (the export spec, R8);
/// without it, the writer itself. `head` is the loop render's head, in
/// the file's rate, and `total` the frames the scratch file holds.
/// Returns the writer, the gain, and the peak of what was written.
pub(crate) fn finish(
    out: Out,
    spec: EncodeSpec,
    partial: &Path,
    scratch: &Path,
    head: &mut [f32],
    total: u64,
    peak: f32,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<(AudioWriter, f64, f32), ExportError> {
    let failed = |what: &Path, error: &dyn std::fmt::Display| {
        ExportError::Failed(format!("{}: {error}", what.display()))
    };
    let Out::Scratch(file, _) = out else {
        return Err(ExportError::Failed(
            "the render has no scratch file to normalise".to_owned(),
        ));
    };
    let mut gain = 1.0;
    if peak > 0.0 {
        gain = 1.0 / f64::from(peak);
    }
    file.into_inner()
        .map_err(|error| failed(scratch, &error.error().to_string()))?;
    let mut writer = AudioWriter::create(partial, spec).map_err(|error| failed(partial, &error))?;
    let channels = usize::from(spec.channels);
    let mut reader = BufReader::with_capacity(
        1 << 16,
        File::open(scratch).map_err(|error| failed(scratch, &error))?,
    );
    let chunk_frames = 8192;
    let mut bytes = vec![0u8; chunk_frames * channels * 4];
    let mut chunk = Vec::with_capacity(chunk_frames * channels);
    let mut read = 0u64;
    let mut peak = 0.0f32;
    for sample in head.iter_mut() {
        *sample = (f64::from(*sample) * gain) as f32;
        peak = peak.max(sample.abs());
    }
    writer
        .write(head)
        .map_err(|error| failed(partial, &error))?;
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
        progress(read as f64 / total.max(1) as f64);
    }
    Ok((writer, gain, peak))
}

fn run(
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    plugins: Hosts<'_>,
    job: &Job,
    (partial, scratch): (&Path, &Path),
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Report, ExportError> {
    let began = Instant::now();
    let failed = |what: &Path, error: &dyn std::fmt::Display| {
        ExportError::Failed(format!("{}: {error}", what.display()))
    };
    let spec = encode_spec(job)?;
    // Spec: export R26. A loop's head can't be written before the tail is
    // known, so it waits in memory, the first TAIL_SECONDS of the file at
    // most (the tail is no longer); the rest goes to the scratch file.
    let staged = job.normalize || job.render_as_loop;
    let channels = usize::from(spec.channels);
    let head_room = if job.render_as_loop {
        TAIL_SECONDS as usize * job.sample_rate as usize * channels
    } else {
        0
    };
    let mut head: Vec<f32> = Vec::new();
    let mut tail: Vec<f32> = Vec::new();
    let mut out = if staged {
        let file = File::create(scratch).map_err(|error| failed(scratch, &error))?;
        Out::Scratch(BufWriter::with_capacity(1 << 16, file), Vec::new())
    } else {
        Out::Writer(AudioWriter::create(partial, spec).map_err(|error| failed(partial, &error))?)
    };

    // Pass one: the render.
    let mut add_tail = |samples: &[f32]| {
        tail.extend_from_slice(samples);
        Ok(())
    };
    let passes = if staged { 2.0 } else { 1.0 };
    let mut peak = 0.0f32;
    let rendered = render(
        document,
        media,
        plugins,
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
            let (first, samples) = samples.split_at((head_room - head.len()).min(samples.len()));
            head.extend_from_slice(first);
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
        job.render_as_loop.then_some(&mut add_tail as &mut Sink<'_>),
    )?;

    // Spec: export R24. Frame j of the tail lands on frame j mod the range's
    // length: the head holds the whole range, or, past TAIL_SECONDS, more
    // frames than the tail has.
    for (j, frame) in tail.chunks_exact(channels).enumerate() {
        let at = j % rendered.frames as usize * channels;
        for (sample, &added) in head[at..at + channels].iter_mut().zip(frame) {
            *sample += added;
        }
    }
    peak = head
        .iter()
        .fold(peak, |peak, sample| peak.max(sample.abs()));

    // Pass two, with Normalize or Render as Loop: the head and the scratch
    // file through the gain to the writer. Spec: export R8.
    let total = rendered.frames - (head.len() / channels) as u64;
    let (writer, gain, peak) = match out {
        Out::Writer(writer) => (writer, 1.0, peak),
        out @ Out::Scratch(_, _) => finish(
            out,
            spec,
            partial,
            scratch,
            &mut head,
            total,
            peak,
            cancel,
            &mut |share| progress(0.5 + share / 2.0),
        )?,
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(ExportError::Cancelled);
    }
    let written = writer.finish().map_err(|error| failed(partial, &error))?;
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

/// Sanitizes a track's name for use in a file name: replaces path separators
/// and illegal characters with `-`, and trims whitespace.
fn sanitize_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'..='\x1f' => '-',
            _ => c,
        })
        .collect();
    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        "Track".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Generates distinct destination file paths for a list of tracks, named
/// beside `base_path` as `<base_stem> - <sanitized_track_name>.<ext>`.
fn track_paths(base_path: &Path, ext: &str, tracks: &[&bl_model::Track]) -> Vec<PathBuf> {
    let parent = match base_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let base_stem = base_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Export");

    let mut used = std::collections::HashSet::new();
    let mut paths = Vec::with_capacity(tracks.len());
    for track in tracks {
        let name = sanitize_name(track.name());
        let mut candidate = parent.join(format!("{base_stem} - {name}.{ext}"));
        let mut count = 2;
        while used.contains(&candidate) {
            candidate = parent.join(format!("{base_stem} - {name} ({count}).{ext}"));
            count += 1;
        }
        used.insert(candidate.clone());
        paths.push(candidate);
    }
    paths
}

/// Whether a track makes audio when exported as an individual track.
///
/// Audio, group, return, and Main tracks make audio. A MIDI track with no
/// instrument and no audio effects is skipped; one with audio effects but no
/// instrument renders as silence.
fn track_makes_audio(track: &bl_model::Track) -> bool {
    match track.kind() {
        bl_model::TrackKind::Audio
        | bl_model::TrackKind::Group
        | bl_model::TrackKind::Return
        | bl_model::TrackKind::Main => true,
        bl_model::TrackKind::Midi => track.devices().any(|d| {
            d.role == bl_model::DeviceRole::Instrument
                || d.role == bl_model::DeviceRole::AudioEffect
        }),
    }
}

/// Prepares a document snapshot for rendering an individual track (WP-2.26).
///
/// By default (`include_return_and_main_effects: false`), renders the track's
/// post-fader signal (its devices, volume, and pan) with nothing from return
/// tracks and nothing from Main's devices or fader:
/// - solos the track exclusively;
/// - routes a grouped track straight to Main;
/// - drops sends and mutes return tracks (or keeps sends for a return track being rendered);
/// - turns Solo in Place off;
/// - bypasses Main's devices;
/// - sets Main's volume to unity (1.0) and pan to centre (0.0).
///
/// When `include_return_and_main_effects` is true, the track renders as it does
/// soloed in the set.
fn prepare_document_for_track(
    document: &Document,
    track_id: TrackId,
    include_return_and_main_effects: bool,
) -> Result<Document, String> {
    if track_id == document.main_id() {
        return Ok(document.clone());
    }

    let track = document
        .track(track_id)
        .ok_or_else(|| format!("no track has the ID {}", track_id.0))?;
    let kind = track.kind();
    let is_grouped = document.parent(track_id).is_some();
    let returns: Vec<TrackId> = document.returns().collect();
    let send_targets: Vec<TrackId> = track.sends().map(|(to, _)| to).collect();
    let main_devices: Vec<DeviceId> = document.main().devices().map(|d| d.id).collect();
    let main_id = document.main_id();

    let mut doc = document.clone();
    doc.apply(&bl_model::Command::SoloExclusive { track: track_id })
        .map_err(|e| e.to_string())?;

    if include_return_and_main_effects {
        return Ok(doc);
    }

    // Route a grouped track straight to Main, bypassing enclosing group devices/fader.
    if is_grouped {
        doc.apply(&bl_model::Command::SetOutput {
            track: track_id,
            output: bl_model::Output::Main,
        })
        .map_err(|e| e.to_string())?;
    }

    // Turn Solo in Place off so solo silences return tracks.
    doc.apply(&bl_model::Command::SetSoloInPlace { enabled: false })
        .map_err(|e| e.to_string())?;

    if kind == bl_model::TrackKind::Return {
        // A return track being exported receives sends from other tracks; silence other returns.
        for ret in returns {
            if ret != track_id {
                doc.apply(&bl_model::Command::SetMute {
                    track: ret,
                    mute: true,
                })
                .map_err(|e| e.to_string())?;
            }
        }
    } else {
        // Audio, MIDI, or group track: mute all return tracks and drop this track's sends.
        for ret in returns {
            doc.apply(&bl_model::Command::SetMute {
                track: ret,
                mute: true,
            })
            .map_err(|e| e.to_string())?;
        }
        for to in send_targets {
            doc.apply(&bl_model::Command::SetSend {
                track: track_id,
                to,
                level: 0.0,
            })
            .map_err(|e| e.to_string())?;
        }
    }

    // Bypass Main's devices.
    for device in main_devices {
        doc.apply(&bl_model::Command::SetDeviceEnabled {
            device,
            enabled: false,
        })
        .map_err(|e| e.to_string())?;
    }

    // Set Main's fader to unity (volume = 1.0, pan = 0.0).
    doc.apply(&bl_model::Command::SetVolume {
        track: main_id,
        volume: 1.0,
    })
    .map_err(|e| e.to_string())?;
    doc.apply(&bl_model::Command::SetPan {
        track: main_id,
        pan: 0.0,
    })
    .map_err(|e| e.to_string())?;

    Ok(doc)
}

/// Runs `export.audio` with its arguments: the set in `document`, its
/// clips' audio from `media`, its plug-ins lent by `plugins`, the
/// Arrangement's time `selection`, the Arrangement's `selected_tracks`,
/// and the engine's rate.
///
/// # Errors
///
/// [`ShellError`] for arguments that can't be exported, an existing file,
/// or an export that failed.
pub fn run_command(
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    plugins: Hosts<'_>,
    selection: Option<(Ticks, Ticks)>,
    selected_tracks: &[TrackId],
    engine_rate: u32,
    args: &ExportArgs,
    audio: Option<&mut crate::audio::AudioHost>,
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
                "{} has no audio file extension; end it in .wav, .aif, .flac, or .mp3, or give \
                 `file_type`",
                args.path
            )));
        }
        (Some(asked), named) => {
            let asked = match asked {
                SpecFileType::Wav => FileType::Wav,
                SpecFileType::Aiff => FileType::Aiff,
                SpecFileType::Flac => FileType::Flac,
                SpecFileType::Mp3 => FileType::Mp3,
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
        SpecDither::NoiseShaped => Dither::NoiseShaped,
    };
    let (start, end, source) =
        range(document, args.start.as_ref(), args.length, selection).map_err(invalid)?;

    // Resolve target tracks (WP-2.26)
    let rendered_choice = args.rendered_track.clone().unwrap_or_default();
    if args.include_return_and_main_effects && rendered_choice == RenderedTrack::Main {
        return Err(invalid(
            "`include_return_and_main_effects` is unavailable when `rendered_track` is Main"
                .to_owned(),
        ));
    }

    let make_job = |dest_path: PathBuf| Job {
        path: dest_path,
        file_type,
        bit_depth,
        dither,
        sample_rate: args.sample_rate,
        engine_rate,
        normalize: args.normalize,
        start,
        length: Length::To(end),
        replace: args.replace,
        render_as_loop: args.render_as_loop,
        block: BLOCK,
    };

    let (renders, written_paths): (Vec<(Option<TrackId>, Document, Job)>, Vec<PathBuf>) =
        match rendered_choice {
        RenderedTrack::Main => {
            let job = make_job(path.clone());
            (vec![(None, document.clone(), job)], vec![path.clone()])
        }
        RenderedTrack::Track(id) => {
            if id == document.main_id() {
                return Err(invalid(
                    "the Main track is the whole mix: choose `main` to render it".to_owned(),
                ));
            }
            if document.track(id).is_none() {
                return Err(invalid(format!("no track has the ID {}", id.0)));
            }
            let transformed =
                prepare_document_for_track(document, id, args.include_return_and_main_effects)
                    .map_err(invalid)?;
            let job = make_job(path.clone());
            (vec![(Some(id), transformed, job)], vec![path.clone()])
        }
        RenderedTrack::AllTracks => {
            let tracks: Vec<&bl_model::Track> = document
                .track_order()
                .into_iter()
                .filter_map(|id| document.track(id))
                .filter(|t| track_makes_audio(t))
                .collect();
            if tracks.is_empty() {
                return Err(invalid(
                    "the set has no tracks to export; add a track or choose Main".to_owned(),
                ));
            }
            let dest_paths = track_paths(&path, file_type.extension(), &tracks);
            let mut renders = Vec::with_capacity(tracks.len());
            for (track, dest) in tracks.iter().zip(&dest_paths) {
                let transformed = prepare_document_for_track(
                    document,
                    track.id(),
                    args.include_return_and_main_effects,
                )
                .map_err(invalid)?;
                renders.push((Some(track.id()), transformed, make_job(dest.clone())));
            }
            (renders, dest_paths)
        }
        RenderedTrack::SelectedTracks => {
            if selected_tracks.is_empty() {
                return Err(invalid(
                    "no tracks are selected in the Arrangement: select tracks, or choose all tracks or Main".to_owned(),
                ));
            }
            let tracks: Vec<&bl_model::Track> = document
                .track_order()
                .into_iter()
                .filter(|id| selected_tracks.contains(id))
                .filter_map(|id| document.track(id))
                .filter(|t| track_makes_audio(t))
                .collect();
            if tracks.is_empty() {
                return Err(invalid(
                    "the selected tracks make no audio: add an instrument or audio effect, or choose Main".to_owned(),
                ));
            }
            let dest_paths = track_paths(&path, file_type.extension(), &tracks);
            let mut renders = Vec::with_capacity(tracks.len());
            for (track, dest) in tracks.iter().zip(&dest_paths) {
                let transformed = prepare_document_for_track(
                    document,
                    track.id(),
                    args.include_return_and_main_effects,
                )
                .map_err(invalid)?;
                renders.push((Some(track.id()), transformed, make_job(dest.clone())));
            }
            (renders, dest_paths)
        }
    };

    // Spec: external-devices R22. The outputs whose path runs through
    // external hardware render in real time; the others render offline, as
    // before.
    let mut live: Vec<(Option<TrackId>, &Document, &Job)> = Vec::new();
    let mut offline: Vec<(&Document, &Job)> = Vec::new();
    for (track, document, job) in &renders {
        if crate::realtime::needs_real_time(document, *track) {
            live.push((*track, document, job));
        } else {
            offline.push((document, job));
        }
    }
    let failed = |error: ExportError| match error {
        ExportError::Invalid(message) => invalid(message),
        ExportError::Exists(_) => ShellError::Unavailable {
            command: AUDIO.to_owned(),
            reason: format!("{error}: set `replace` to replace it"),
        },
        ExportError::Cancelled | ExportError::Failed(_) => ShellError::Failed {
            command: AUDIO.to_owned(),
            message: error.to_string(),
        },
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let (mut reports, partials) = render_multi(&offline, media, plugins, &cancel, &mut |_| {})
        .map_err(failed)?;
    let mut outcome = crate::realtime::Outcome::default();
    if !live.is_empty() {
        // Spec: external-devices R22, R26. Without a running audio device,
        // an export that needs real time is refused before anything renders.
        let Some(audio) = audio else {
            return Err(invalid(
                "the export needs the hardware, and no audio device is open".to_owned(),
            ));
        };
        let jobs: Vec<(Option<TrackId>, Job)> = live
            .iter()
            .map(|(track, _, job)| (*track, (*job).clone()))
            .collect();
        let (live_reports, live_outcome) = crate::realtime::render(
            audio,
            &jobs,
            document,
            start,
            Length::To(end),
            std::time::Duration::from_secs(u64::from(args.wait_seconds.unwrap_or(10))),
            args.auto_restart.unwrap_or(true),
            args.max_attempts,
            &cancel,
            &mut |_| {},
        )
        .map_err(failed)?;
        reports.extend(live_reports);
        outcome = live_outcome;
        let live_partials: Vec<PathBuf> = jobs
            .iter()
            .map(|(_, job)| hidden_beside(&job.path, "partial"))
            .collect();
        let destinations: Vec<PathBuf> = jobs.iter().map(|(_, job)| job.path.clone()).collect();
        promote(&live_partials, &destinations).map_err(failed)?;
        let _ = partials;
    } else {
        let destinations: Vec<PathBuf> = offline.iter().map(|(_, job)| job.path.clone()).collect();
        promote(&partials, &destinations).map_err(failed)?;
    }

    let canonical_path = fs::canonicalize(&path).unwrap_or(path);
    let files: Vec<String> = written_paths
        .iter()
        .map(|p| {
            fs::canonicalize(p)
                .unwrap_or_else(|_| p.clone())
                .display()
                .to_string()
        })
        .collect();

    let first_report = reports.first().cloned().unwrap_or_default();
    let max_peak = reports.iter().map(|r| r.peak).fold(0.0f32, f32::max);
    let total_took: Duration = reports.iter().map(|r| r.took).sum();
    let total_frames: u64 = reports.iter().map(|r| r.frames).sum();
    let mut missing_media = Vec::new();
    for r in &reports {
        for m in &r.missing_media {
            let s = m.to_string();
            if !missing_media.contains(&s) {
                missing_media.push(s);
            }
        }
    }

    let speed = if total_took.as_secs_f64() > 0.0 {
        (total_frames as f64 / f64::from(args.sample_rate.max(1))) / total_took.as_secs_f64()
    } else {
        0.0
    };

    Ok(ExportResult {
        path: canonical_path.display().to_string(),
        files,
        file_type: match file_type {
            FileType::Wav => SpecFileType::Wav,
            FileType::Aiff => SpecFileType::Aiff,
            FileType::Flac => SpecFileType::Flac,
            FileType::Mp3 => SpecFileType::Mp3,
        },
        bit_depth: args.bit_depth,
        sample_rate: args.sample_rate,
        channels: first_report.channels,
        frames: first_report.frames,
        seconds: first_report.frames as f64 / f64::from(args.sample_rate),
        start,
        end,
        range: source,
        dither: if bit_depth.is_integer() && file_type != FileType::Mp3 {
            args.dither.unwrap_or_default()
        } else {
            SpecDither::None
        },
        peak_dbfs: (max_peak > 0.0).then(|| 20.0 * f64::from(max_peak).log10()),
        gain_db: 20.0 * first_report.gain.log10(),
        render_rate: first_report.render_rate,
        took_seconds: total_took.as_secs_f64(),
        speed,
        missing_media,
        // Spec: external-devices R16. An offline export reaches no hardware.
        silenced_external_devices: document.external_devices(),
        // Spec: external-devices R26, R27. The export says whether it
        // rendered in real time, how many attempts it took, and the
        // dropouts it found.
        real_time: !live.is_empty(),
        attempts: outcome.attempts,
        dropouts: outcome
            .dropouts
            .iter()
            .map(|dropout| SpecDropout {
                position: dropout.position,
                kind: dropout.kind.clone(),
            })
            .collect(),
    })
}

/// Converts a render down to the file's rate as it arrives, a chunk at a
/// time, exactly as [`RateConverter::convert`] converts it whole: the
/// converter's delay dropped, and `ceil(frames × to ÷ from)` frames out.
pub(crate) struct Downsampler {
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
    pub(crate) fn new(
        from: u32,
        to: u32,
        channels: usize,
        frames: u64,
    ) -> Result<Self, ExportError> {
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

    pub(crate) fn push(
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

    pub(crate) fn finish(
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
