//! `bl-cli render`: a set played from its start and written to a WAV file,
//! faster than real time, with no audio device.
//!
//! The song plays through [`bl_engine::Offline`], which runs the same
//! processor a device's audio thread runs, one block after another on this
//! thread. So a render is what the engine plays, and nothing in it depends
//! on timing: the same set renders to the same samples every time, at any
//! block size. The set's clips play their audio files through the media
//! store, which finds each file by the path in the set's media table,
//! relative to the project folder, and checks it still holds its content.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use bl_core::time::{SampleRate, Ticks};
use bl_engine::{
    EngineConfig, MediaLibrary, MediaMap, MediaStreams, Offline, Output, Registry, Report,
    ScheduledCommand, TransportCommand, When,
};
use bl_media::MediaStore;
use bl_model::{ContentHash, DeviceId, Document};
use bl_project::{Project, ProjectFolder};

use crate::Outcome;

/// How long `bl-cli render` renders a set without clips when neither
/// `--seconds` nor `--bars` says. A set with clips renders to the end of its
/// last clip.
pub const DEFAULT_SECONDS: f64 = 10.0;

/// The longest render `bl-cli render` makes: a day.
const MAX_SECONDS: f64 = 86_400.0;

/// The largest block `bl-cli render` accepts, in frames.
const MAX_BLOCK: usize = 8192;

/// How to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The sample rate.
    pub sample_rate: SampleRate,
    /// The engine's block size in frames, at least 1. The output is the same
    /// at any size.
    pub block: usize,
    /// How many frames to render.
    pub frames: u64,
}

/// What a render did, apart from its audio.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// Frames rendered.
    pub frames: u64,
    /// Channels in each frame.
    pub channels: usize,
    /// The largest absolute sample value, among finite samples.
    pub peak: f32,
    /// Samples that are NaN or infinite. Any is a bug.
    pub non_finite: u64,
    /// Devices no factory makes a node for, whose tracks' audio passes
    /// through where they'd be.
    pub unhosted: Vec<DeviceId>,
    /// Media that clips name and the media library couldn't supply, whose
    /// clips played silence.
    pub missing_media: Vec<ContentHash>,
    /// What the engine reported: the scheduled commands that fired, and any
    /// it had no room for.
    pub reports: Vec<Report>,
    /// Heap operations in the engine's blocks, counted where the allocation
    /// trap is installed (debug and test builds). The real-time rules allow
    /// none.
    pub heap_operations: u64,
}

impl Summary {
    /// The peak in dBFS.
    #[must_use]
    pub fn peak_dbfs(&self) -> f64 {
        dbfs(f64::from(self.peak))
    }

    /// How many scheduled commands fired.
    #[must_use]
    pub fn fired(&self) -> usize {
        self.reports
            .iter()
            .filter(|report| matches!(report, Report::Fired { .. }))
            .count()
    }
}

/// A transport command on a frame of the render: the transport moves as a
/// person or an agent would move it, on that exact frame, whatever the
/// block size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cue {
    /// The frame, from the render's first.
    pub at: u64,
    /// What the transport does there.
    pub command: TransportCommand,
}

impl Cue {
    /// Play from the start: what `bl-cli render` does.
    pub const PLAY: Self = Self {
        at: 0,
        command: TransportCommand::Play,
    };
}

/// Plays `document` from its start, with `schedule`'s commands scheduled
/// before the first block, and hands each block's output to `sink`. It has
/// no media, so any clips play silence: [`render_with`] gives them some.
///
/// # Panics
///
/// If `settings.block` is 0.
pub fn render(
    document: &Document,
    schedule: &[ScheduledCommand],
    settings: &Settings,
    sink: impl FnMut(&Output<'_>),
) -> Summary {
    render_cued(document, schedule, &[Cue::PLAY], settings, sink)
}

/// Renders `document` with `schedule`'s commands scheduled and the
/// transport moved by `cues`, each on its frame, and hands each block's
/// output to `sink`. The song starts stopped, with its insert marker at
/// 1.1.1: a render that plays has a cue that starts it. It has no media, so
/// any clips play silence: [`render_with`] gives them some.
///
/// # Panics
///
/// If `settings.block` is 0.
pub fn render_cued(
    document: &Document,
    schedule: &[ScheduledCommand],
    cues: &[Cue],
    settings: &Settings,
    sink: impl FnMut(&Output<'_>),
) -> Summary {
    let none: Arc<dyn MediaLibrary> = Arc::new(MediaMap::new());
    render_with(document, none, schedule, cues, settings, sink)
}

/// [`render_cued`], with the clips' audio from `media`.
///
/// # Panics
///
/// If `settings.block` is 0.
pub fn render_with(
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    schedule: &[ScheduledCommand],
    cues: &[Cue],
    settings: &Settings,
    mut sink: impl FnMut(&Output<'_>),
) -> Summary {
    let config = EngineConfig::new(settings.sample_rate, settings.block);
    let mut offline = Offline::new(config, Registry::new().with_media(media));
    let unhosted = offline.sync(document);
    let missing_media = offline.missing_media().to_vec();
    for cue in cues {
        offline
            .sender()
            .transport_at(cue.command, When::Frame(cue.at));
    }
    for &command in schedule {
        offline.sender().schedule(command);
    }
    let mut summary = Summary {
        channels: config.channels,
        unhosted,
        missing_media,
        ..Summary::default()
    };
    offline.render(settings.frames, |output| {
        for c in 0..output.channels() {
            for &sample in output.channel(c) {
                if sample.is_finite() {
                    summary.peak = summary.peak.max(sample.abs());
                } else {
                    summary.non_finite += 1;
                }
            }
        }
        summary.frames += output.frames() as u64;
        sink(&output);
    });
    summary.reports = offline.reports();
    summary.heap_operations = offline.heap_operations();
    summary
}

/// Appends a block to `samples`, interleaved: each frame's channels in turn.
pub fn interleave(output: &Output<'_>, samples: &mut Vec<f32>) {
    samples.reserve(output.frames() * output.channels());
    for frame in 0..output.frames() {
        samples.extend((0..output.channels()).map(|c| output.channel(c)[frame]));
    }
}

/// A sample as a 24-bit integer: scaled so that 1.0 is 2²³, rounded to the
/// nearest step (halves away from zero), and clipped to the 24-bit range.
/// NaN gives 0.
#[must_use]
pub fn to_i24(sample: f32) -> i32 {
    const SCALE: f64 = 8_388_608.0;
    // Scaling by a power of two is exact, so this is the one rounding.
    (f64::from(sample) * SCALE)
        .round()
        .clamp(-SCALE, SCALE - 1.0) as i32
}

/// A linear amplitude in dBFS: −∞ for silence.
#[must_use]
pub fn dbfs(amplitude: f64) -> f64 {
    20.0 * amplitude.log10()
}

/// The sample format of the WAV file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// 32-bit float: the engine's output exactly.
    F32,
    /// 24-bit integer, rounded to the nearest step, without dither.
    I24,
}

impl Format {
    fn describe(self) -> &'static str {
        match self {
            Self::F32 => "32-bit float",
            Self::I24 => "24-bit integer",
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct RenderArgs {
    /// The set to render: a `.blset` file, or a project folder holding one
    /// set.
    pub project: PathBuf,

    /// The WAV file to write. An existing file is replaced.
    pub output: PathBuf,

    /// How long to render, in seconds. Defaults to the end of the set's
    /// last clip, or 10 seconds in a set without clips, unless `--bars` is
    /// given.
    #[arg(long, conflicts_with = "bars", allow_negative_numbers = true)]
    pub seconds: Option<f64>,

    /// How long to render, in bars, by the set's time signatures and tempo.
    #[arg(long)]
    pub bars: Option<u32>,

    /// The sample rate in hertz.
    #[arg(long, default_value_t = 48_000)]
    pub rate: u32,

    /// The engine's block size in frames. The output is the same at any
    /// size.
    #[arg(long, default_value_t = 512)]
    pub block: usize,

    /// The sample format.
    #[arg(long, value_enum, default_value_t = Format::F32)]
    pub format: Format,
}

/// Runs `bl-cli render`.
///
/// # Errors
///
/// A message if an argument is out of range, the set doesn't load, or the
/// WAV file can't be written. The exit code is a failure if the render had
/// a sample that isn't finite, or (in a build with the allocation trap)
/// touched the heap inside a block.
pub fn run(args: &RenderArgs) -> Outcome {
    let sample_rate = SampleRate::new(args.rate)
        .filter(|_| (8_000..=384_000).contains(&args.rate))
        .ok_or("--rate must be from 8000 to 384000 Hz")?;
    if !(1..=MAX_BLOCK).contains(&args.block) {
        return Err(format!("--block must be from 1 to {MAX_BLOCK} frames"));
    }
    let is_wav = args
        .output
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"));
    if !is_wav {
        return Err("OUTPUT must be a .wav file".into());
    }
    let (project, info) = bl_project::load(&args.project)
        .map_err(|error| format!("can't open {}: {error}", args.project.display()))?;
    let document = &project.document;
    let frames = length(document, args.seconds, args.bars, sample_rate)?;
    let settings = Settings {
        sample_rate,
        block: args.block,
        frames,
    };
    let folder = if args.project.is_dir() {
        ProjectFolder::new(&args.project)
    } else {
        ProjectFolder::of_set(&args.project)
    };
    let media = media_store(&project, folder.root());

    let began = Instant::now();
    let summary = write_wav(&args.output, document, media, &settings, args.format)?;
    let took = began.elapsed().as_secs_f64();
    let seconds = frames as f64 / sample_rate.as_f64();

    println!(
        "set:       {} (revision {}, saved by {})",
        args.project.display(),
        info.revision,
        info.saved_by
    );
    println!(
        "length:    {seconds:.3} s: {frames} frames at {} Hz, {} channels",
        sample_rate.hz(),
        summary.channels
    );
    println!(
        "rendered:  in {took:.3} s, {:.0} times real time, in blocks of {} frames",
        seconds / took.max(1e-9),
        args.block
    );
    println!("peak:      {:.1} dBFS", summary.peak_dbfs());
    println!(
        "wrote:     {} ({})",
        args.output.display(),
        args.format.describe()
    );
    if bl_rt::trap::is_installed() {
        println!(
            "heap operations: {} in the engine's blocks (allocation trap on)",
            summary.heap_operations
        );
    }
    if !summary.missing_media.is_empty() {
        let hashes: Vec<String> = summary
            .missing_media
            .iter()
            .map(ToString::to_string)
            .collect();
        eprintln!(
            "warning: {} audio file{} missing or changed, so {} clips play silence: {}",
            hashes.len(),
            if hashes.len() == 1 { " is" } else { "s are" },
            if hashes.len() == 1 { "its" } else { "their" },
            hashes.join(", ")
        );
    }
    if !summary.unhosted.is_empty() {
        let ids: Vec<String> = summary.unhosted.iter().map(ToString::to_string).collect();
        eprintln!(
            "warning: no node plays {} device{} in this build ({}); audio passes through where {} would be",
            ids.len(),
            if ids.len() == 1 { "" } else { "s" },
            ids.join(", "),
            if ids.len() == 1 { "it" } else { "they" }
        );
    }

    let mut failed = false;
    if summary.non_finite > 0 {
        eprintln!(
            "error: {} samples in the render are NaN or infinite",
            summary.non_finite
        );
        failed = true;
    }
    if summary.heap_operations > 0 {
        eprintln!(
            "error: the engine touched the heap {} times inside its blocks",
            summary.heap_operations
        );
        failed = true;
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// `project`'s media table, each file's path resolved against `folder`,
/// for an offline render: long files stream from disk in blocking mode, so
/// memory follows the streams' buffers rather than the files' lengths and
/// the render is the same every time (the disk streaming spec, R8). Nothing
/// is read until a clip needs its audio. If the disk thread can't start,
/// files decode whole into memory instead.
#[must_use]
pub fn media_store(project: &Project, folder: &Path) -> Arc<dyn MediaLibrary> {
    let store = MediaStore::new();
    for file in project.media.iter() {
        if let Some(path) = file.path.resolve(folder) {
            store.add_reference(
                bl_media::ContentHash::from_bytes(file.hash.0),
                path,
                file.size,
            );
        }
    }
    let store = Arc::new(store);
    match MediaStreams::blocking(Arc::clone(&store)) {
        Ok(streams) => Arc::new(streams),
        Err(_) => store,
    }
}

/// Where the song ends: the end of its last clip, if it has clips.
#[must_use]
pub fn song_end(document: &Document) -> Option<Ticks> {
    document
        .tracks()
        .flat_map(bl_model::Track::clips)
        .map(bl_model::Clip::end)
        .max()
}

/// The number of frames to render.
fn length(
    document: &Document,
    seconds: Option<f64>,
    bars: Option<u32>,
    rate: SampleRate,
) -> Result<u64, String> {
    let settings = document.settings();
    let frames = if let Some(bars) = bars {
        if bars == 0 {
            return Err("--bars must be at least 1".into());
        }
        let end = settings.time_signatures.bar_start(i64::from(bars));
        settings.tempo.ticks_to_frames(end, rate).get()
    } else if let (None, Some(end)) = (seconds, song_end(document)) {
        settings.tempo.ticks_to_frames(end, rate).get()
    } else {
        let seconds = seconds.unwrap_or(DEFAULT_SECONDS);
        if !(seconds > 0.0 && seconds <= MAX_SECONDS) {
            return Err(format!(
                "--seconds must be more than 0 and at most {MAX_SECONDS}"
            ));
        }
        rate.seconds_to_frames(seconds).get()
    };
    let max = rate.seconds_to_frames(MAX_SECONDS).get();
    if frames > max {
        return Err(format!(
            "that's longer than the longest render, {MAX_SECONDS} seconds"
        ));
    }
    Ok(frames.max(1) as u64)
}

/// Renders `document` into a WAV file at `path`, with its clips' audio from
/// `media`. The file is written beside it under a temporary name and
/// renamed into place when it's complete, so a failure leaves no partial
/// file at `path`.
fn write_wav(
    path: &Path,
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    settings: &Settings,
    format: Format,
) -> Result<Summary, String> {
    let partial = partial_path(path);
    let result = write_wav_to(&partial, document, media, settings, format).and_then(|summary| {
        fs::rename(&partial, path)
            .map(|()| summary)
            .map_err(|error| format!("can't write {}: {error}", path.display()))
    });
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

fn write_wav_to(
    path: &Path,
    document: &Document,
    media: Arc<dyn MediaLibrary>,
    settings: &Settings,
    format: Format,
) -> Result<Summary, String> {
    let failed = |error: hound::Error| format!("can't write {}: {error}", path.display());
    let spec = hound::WavSpec {
        // The engine's channel count, which `EngineConfig::new` sets.
        channels: EngineConfig::new(settings.sample_rate, settings.block).channels as u16,
        sample_rate: settings.sample_rate.hz(),
        bits_per_sample: match format {
            Format::F32 => 32,
            Format::I24 => 24,
        },
        sample_format: match format {
            Format::F32 => hound::SampleFormat::Float,
            Format::I24 => hound::SampleFormat::Int,
        },
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(failed)?;
    let mut error = None;
    let summary = render_with(document, media, &[], &[Cue::PLAY], settings, |output| {
        if error.is_some() {
            return;
        }
        for frame in 0..output.frames() {
            for c in 0..output.channels() {
                let sample = output.channel(c)[frame];
                let written = match format {
                    Format::F32 => writer.write_sample(sample),
                    Format::I24 => writer.write_sample(to_i24(sample)),
                };
                if let Err(e) = written {
                    error = Some(e);
                    return;
                }
            }
        }
    });
    if let Some(error) = error {
        return Err(failed(error));
    }
    writer.finalize().map_err(failed)?;
    Ok(summary)
}

/// `.<name>.partial` beside `path`.
fn partial_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(".partial");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_round_to_the_nearest_24_bit_step_and_clip() {
        assert_eq!(to_i24(0.0), 0);
        assert_eq!(to_i24(0.5), 1 << 22);
        assert_eq!(to_i24(-1.0), -(1 << 23));
        assert_eq!(to_i24(1.0), (1 << 23) - 1);
        assert_eq!(to_i24(2.0), (1 << 23) - 1);
        assert_eq!(to_i24(1.5 / 8_388_608.0), 2, "halves round away from zero");
        assert_eq!(to_i24(-1.5 / 8_388_608.0), -2);
        assert_eq!(to_i24(f32::NAN), 0);
    }

    #[test]
    fn a_length_in_bars_follows_the_tempo_and_time_signatures() {
        let document = bl_model::Editor::new().document().clone();
        // Two bars of 4/4 at 120 BPM.
        assert_eq!(
            length(&document, None, Some(2), SampleRate::HZ_48000),
            Ok(192_000)
        );
        assert_eq!(
            length(&document, Some(1.5), None, SampleRate::HZ_48000),
            Ok(72_000)
        );
        assert_eq!(
            length(&document, None, None, SampleRate::HZ_48000),
            Ok(480_000)
        );
        assert!(length(&document, Some(0.0), None, SampleRate::HZ_48000).is_err());
        assert!(length(&document, Some(f64::NAN), None, SampleRate::HZ_48000).is_err());
        assert!(length(&document, None, Some(0), SampleRate::HZ_48000).is_err());
    }

    #[test]
    fn a_set_with_clips_renders_to_the_end_of_its_last_clip() {
        use bl_model::{AudioData, Clip, ClipData, Command, Editor, TrackKind};

        let mut editor = Editor::new();
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        for (start, length) in [
            (Ticks::QUARTER * 3, Ticks::QUARTER),
            (Ticks::ZERO, Ticks::QUARTER),
        ] {
            let data = ClipData::Audio(AudioData::new(ContentHash([1; 32])));
            let clip = Clip::new(editor.new_id(), start, length, data);
            editor.apply(Command::CreateClip { track, clip }).unwrap();
        }
        let document = editor.document();
        assert_eq!(song_end(document), Some(Ticks::WHOLE));
        // Four beats at 120 BPM: two seconds, unless told otherwise.
        assert_eq!(
            length(document, None, None, SampleRate::HZ_48000),
            Ok(96_000)
        );
        assert_eq!(
            length(document, Some(1.0), None, SampleRate::HZ_48000),
            Ok(48_000)
        );
    }

    #[test]
    fn the_partial_file_is_hidden_beside_the_output() {
        assert_eq!(
            partial_path(Path::new("renders/song.wav")),
            Path::new("renders/.song.wav.partial")
        );
    }
}
