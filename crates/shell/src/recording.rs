//! Recording armed tracks' inputs into the project (WP-2.13).
//!
//! Turning recording on gives the engine a [`RecordingPlan`]: a ring per
//! armed track and where its input's samples go. The engine's audio thread
//! copies them there, one part of a block at a time, with nothing allocated
//! or locked; a writer thread here drains the rings into WAVs in the
//! project's `Samples/Recorded/`, one per track. When the take ends, the
//! engine drops the plan, which closes the rings; the writer finishes the
//! files and reports them, and the shell makes the clips from them
//! ([`crate::Shell`]).
//!
//! A ring that fills up (a writer that fell behind) is reported by the
//! engine's frames-dropped count, and a write that fails (a full disk) by
//! the writer's report: neither is ever silent, and a failed recording
//! leaves no partial file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime};

use bl_core::time::SampleRate;
use bl_engine::{RecordTrack, RecordingPlan, TakeBounds};
use bl_media::{AudioWriter, BitDepth, Dither, EncodeSpec, FileType};
use bl_model::{Channel, TrackId};
use rtrb::{Consumer, RingBuffer};

/// Frames of one track's samples the ring between the audio thread and the
/// writer holds, about 2.7 seconds of stereo at 48 kHz, so a writer that
/// stalls briefly loses nothing.
const RING_FRAMES: usize = 1 << 17;

/// Samples the writer buffers before a write.
const CHUNK: usize = 8_192;

/// How long the writer sleeps when the rings are empty.
const POLL: Duration = Duration::from_millis(1);

/// The undo step a finished recording makes. Live's name for it isn't
/// observed yet.
pub const UNDO_LABEL: &str = "Recorded Clip";

/// One track to record: where its input comes from, and the name of the
/// file it goes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// The track whose input is recorded.
    pub track: TrackId,
    /// The interface channels its Audio From names.
    pub channel: Channel,
    /// The file's name, without a directory ([`file_name`]).
    pub name: String,
}

/// The file name Live gives a recording: `<label> <take> [<stamp>].wav` with
/// the stamp of a backup's names, and characters that can't be in a file
/// name replaced by `-`. The label is the track's own (`5-Guitar`).
#[must_use]
pub fn file_name(label: &str, take: u32, stamp: &str) -> String {
    let clean: String = label
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_end_matches(['.', ' ']);
    let clean = if clean.is_empty() { "Audio" } else { clean };
    format!("{clean} {take:04} [{stamp}].wav")
}

/// The stamp recording file names carry, now, in UTC.
#[must_use]
pub fn stamp() -> String {
    bl_project::utc_stamp(SystemTime::now())
}

/// Starts a recording into `folder`, the project's root: one ring per track
/// for the engine, and the writer thread draining them into WAVs under
/// `Samples/Recorded/`, 32-bit float at `rate`.
///
/// # Errors
///
/// A message if `Samples/Recorded` can't be made or the writer thread can't
/// start.
pub fn start(
    folder: &Path,
    rate: SampleRate,
    tracks: &[Track],
) -> Result<(RecordingPlan, Recording), String> {
    let directory = folder.join("Samples").join("Recorded");
    std::fs::create_dir_all(&directory)
        .map_err(|e| format!("couldn't make {}: {e}", directory.display()))?;
    let mut plan = Vec::with_capacity(tracks.len());
    let mut writing = Vec::with_capacity(tracks.len());
    let mut dropped = Vec::with_capacity(tracks.len());
    for track in tracks {
        let channels = track.channels();
        let (producer, consumer) = RingBuffer::new(RING_FRAMES * channels);
        let frames = Arc::new(AtomicU64::new(0));
        dropped.push(Arc::clone(&frames));
        writing.push(Writing {
            consumer,
            path: directory.join(&track.name),
            track: track.track,
            channels,
        });
        plan.push(RecordTrack {
            track: track.track,
            channel: track.channel,
            producer,
            dropped: frames,
        });
    }
    let (done, finished) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("bl-shell-recording".into())
        .spawn(move || write(writing, rate, done))
        .map_err(|e| format!("couldn't start the recording writer: {e}"))?;
    drop(thread);
    Ok((
        RecordingPlan {
            tracks: plan,
            delay: 0,
        },
        Recording {
            finished,
            files: None,
            dropped,
            take: None,
        },
    ))
}

impl Track {
    /// How many samples one frame has: one for a mono channel, two for a
    /// pair.
    #[must_use]
    pub fn channels(&self) -> usize {
        match self.channel {
            Channel::Mono(_) => 1,
            Channel::Pair(_) => 2,
        }
    }
}

/// One track's side of the recording, on the main thread: the writer's
/// report, the frames dropped, and the take the engine published.
pub struct Recording {
    finished: Receiver<Result<Vec<RecordedFile>, String>>,
    /// The writer's report, kept while the engine's take state catches up.
    files: Option<Result<Vec<RecordedFile>, String>>,
    dropped: Vec<Arc<AtomicU64>>,
    take: Option<TakeBounds>,
}

/// What a finished recording came to.
#[derive(Debug)]
pub struct Outcome {
    /// The take the engine published, if it saw one: the song frames the
    /// clips are placed from. `None` when nothing was played.
    pub take: Option<TakeBounds>,
    /// The finished files, or why the writer gave up.
    pub files: Result<Vec<RecordedFile>, String>,
    /// Frames a ring was too full to take: the disk fell behind, and the
    /// recording ended before them.
    pub dropped: u64,
}

/// One file the writer finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedFile {
    /// The track it holds.
    pub track: TrackId,
    /// The file.
    pub path: PathBuf,
    /// The frames written.
    pub frames: u64,
    /// Its channels.
    pub channels: usize,
}

impl Recording {
    /// Notes the take the engine published, from its state.
    pub fn note_take(&mut self, take: TakeBounds) {
        self.take = Some(take);
    }

    /// What the recording came to, once it has: the writer has reported and
    /// the engine's take state has caught up. `None` while either is still
    /// on its way, so the caller polls again. `running` is whether an engine
    /// still runs: with none, the take can't be published any more, and
    /// what the writer finished is reported as it is.
    #[must_use]
    pub fn poll(&mut self, running: bool) -> Option<Outcome> {
        if self.files.is_none() {
            match self.finished.try_recv() {
                Ok(files) => self.files = Some(files),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    self.files = Some(Err("the recording writer stopped".to_owned()));
                }
            }
        }
        let recorded = matches!(&self.files, Some(Ok(files)) if !files.is_empty());
        if recorded && self.take.is_none() && running {
            // The take state is published at the end of the block the plan
            // went in, so it can arrive a frame after the writer's report.
            return None;
        }
        let dropped = self.dropped.iter().map(|d| d.load(Relaxed)).sum();
        Some(Outcome {
            take: self.take,
            files: self.files.take().expect("set above"),
            dropped,
        })
    }
}

impl std::fmt::Debug for Recording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recording")
            .field("dropped", &self.dropped.len())
            .field("take", &self.take)
            .finish_non_exhaustive()
    }
}

/// One track's writer: its ring's reading end, the file to write, and what
/// it writes.
struct Writing {
    consumer: Consumer<f32>,
    path: PathBuf,
    track: TrackId,
    channels: usize,
}

/// The writer thread: drains each track's ring into its file until the
/// engine drops the plan (the take is over), then finishes the files and
/// reports them. Real-time safety binds the audio thread only; here a file
/// write may block as long as it likes.
fn write(
    mut tracks: Vec<Writing>,
    rate: SampleRate,
    done: Sender<Result<Vec<RecordedFile>, String>>,
) {
    let mut writers: Vec<Option<AudioWriter>> = (0..tracks.len()).map(|_| None).collect();
    let mut buffers: Vec<Vec<f32>> = tracks.iter().map(|_| Vec::with_capacity(CHUNK)).collect();
    let mut error: Option<String> = None;
    loop {
        let mut idle = true;
        for (index, track) in tracks.iter_mut().enumerate() {
            let channels = track.channels;
            let whole = CHUNK - CHUNK % channels;
            while let Ok(sample) = track.consumer.pop() {
                idle = false;
                if error.is_some() {
                    // The recording failed: keep draining, so a ring never
                    // fills and has the audio thread drop frames too.
                    continue;
                }
                if writers[index].is_none() {
                    match writer(&track.path, track.channels, rate) {
                        Ok(opened) => writers[index] = Some(opened),
                        Err(e) => {
                            error = Some(e);
                            continue;
                        }
                    }
                }
                buffers[index].push(sample);
                if buffers[index].len() >= whole {
                    let write = writers[index]
                        .as_mut()
                        .expect("opened above")
                        .write(&buffers[index]);
                    if let Err(e) = write {
                        error = Some(format!("{}: {e}", track.path.display()));
                    }
                    buffers[index].clear();
                }
            }
        }
        if tracks.iter().all(|track| track.consumer.is_abandoned()) && idle {
            break;
        }
        thread::sleep(POLL);
    }
    let mut files = Vec::with_capacity(tracks.len());
    for (index, track) in tracks.iter_mut().enumerate() {
        if error.is_some() {
            break;
        }
        let Some(mut writer) = writers[index].take() else {
            // The take never reached this track: no file.
            continue;
        };
        if !buffers[index].is_empty()
            && let Err(e) = writer.write(&buffers[index])
        {
            error = Some(format!("{}: {e}", track.path.display()));
            break;
        }
        match writer.finish() {
            Ok(frames) => files.push(RecordedFile {
                track: track.track,
                path: track.path.clone(),
                frames,
                channels: track.channels,
            }),
            Err(e) => error = Some(format!("{}: {e}", track.path.display())),
        }
    }
    match error {
        Some(error) => {
            // A failed recording leaves no partial file, as a failed export
            // leaves none.
            for track in &tracks {
                let _ = std::fs::remove_file(&track.path);
            }
            let _ = done.send(Err(error));
        }
        None => {
            let _ = done.send(Ok(files));
        }
    }
}

/// A writer for one track's file: WAV, 32-bit float, so the recorded
/// samples are the interface's to the bit.
fn writer(path: &Path, channels: usize, rate: SampleRate) -> Result<AudioWriter, String> {
    AudioWriter::create(
        path,
        EncodeSpec {
            file_type: FileType::Wav,
            bit_depth: BitDepth::Float32,
            dither: Dither::None,
            sample_rate: rate.hz(),
            channels: channels as u16,
        },
    )
    .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_lives_and_hold_no_path_characters() {
        assert_eq!(
            file_name("5-Guitar", 1, "2026-10-15 180000"),
            "5-Guitar 0001 [2026-10-15 180000].wav"
        );
        assert_eq!(
            file_name("2-Bass / Pad", 12, "2026-10-15 180000"),
            "2-Bass - Pad 0012 [2026-10-15 180000].wav"
        );
        assert_eq!(
            file_name("   ", 1, "2026-10-15 180000"),
            "Audio 0001 [2026-10-15 180000].wav"
        );
    }

    // Spec: app-shell R13; WP-2.13. A recording that ends with frames the
    // engine dropped reports them, and a take that never started makes no
    // file.
    #[test]
    fn a_recordings_report_carries_its_dropped_frames() {
        let dir = std::env::temp_dir().join(format!(
            "bl-shell-recording-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let track = TrackId(bl_core::id::Id::new(1).unwrap());
        let (plan, mut recording) = start(
            &dir,
            SampleRate::HZ_48000,
            &[Track {
                track,
                channel: Channel::Mono(3),
                name: file_name("1-Audio", 1, "2026-10-15 180000"),
            }],
        )
        .unwrap();
        let dropped = Arc::clone(&plan.tracks[0].dropped);
        // The take ends (the plan goes), and the engine had dropped frames.
        drop(plan);
        dropped.fetch_add(9, Relaxed);
        let outcome = loop {
            if let Some(outcome) = recording.poll(false) {
                break outcome;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(outcome.dropped, 9);
        assert_eq!(outcome.files.unwrap(), Vec::new(), "no samples, no file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
