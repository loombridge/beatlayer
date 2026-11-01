//! Stem separation's background work (IMP-11): the separation itself, and
//! the thread machinery the Packages manager's downloads share, each on a
//! thread of its own. The shell starts them and lands their results
//! (`Shell::run` for `stems.*`); the behaviour is in the stem separation
//! spec.
//!
//! Its packages are ONNX Runtime's shared library and HTDemucs exported to
//! ONNX: the roles `onnx-runtime` and `stems-model` of the `stems` feature
//! in [`crate::packages`]'s manifest.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;

use bl_analysis::stems::{self as separation, Quality, RATE, Stem};
use bl_media::{ContentHash, MediaStore};

pub use crate::packages::Package;
use crate::packages::manifest;

/// The feature's ID in the manifest.
pub const FEATURE: &str = "stems";

/// ONNX Runtime for this platform, if it has one.
#[must_use]
pub fn runtime() -> Option<Package> {
    manifest().for_role("onnx-runtime", None)
}

/// HTDemucs, exported to ONNX.
///
/// # Panics
///
/// If the manifest in use has no package for the role.
#[must_use]
pub fn model() -> Package {
    manifest()
        .for_role("stems-model", None)
        .expect("the manifest has a package for the stems model")
}

/// The packages stem separation needs, in the order they're downloaded;
/// empty on a platform ONNX Runtime doesn't publish a library for.
#[must_use]
pub fn packages() -> Vec<Package> {
    manifest().feature_packages(FEATURE, None)
}

/// What a download or separation is doing now, shared with its thread.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub(crate) done: Arc<AtomicU64>,
    pub(crate) total: Arc<AtomicU64>,
    stage: Arc<Mutex<String>>,
    /// What a separation runs on, once its model is loaded.
    provider: Arc<Mutex<String>>,
    cancel: Arc<AtomicBool>,
}

impl Progress {
    /// From 0 to 1.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32
    }

    /// What it's doing now.
    #[must_use]
    pub fn stage(&self) -> String {
        self.stage.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// The hardware a separation runs on, and why it isn't the GPU when it
    /// should have been (R33); empty until the model is loaded.
    #[must_use]
    pub fn provider(&self) -> String {
        self.provider.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Asks the thread to stop.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn set_provider(&self, provider: String) {
        if let Ok(mut p) = self.provider.lock() {
            *p = provider;
        }
    }

    pub(crate) fn set_stage(&self, stage: impl Into<String>) {
        if let Ok(mut s) = self.stage.lock() {
            *s = stage.into();
        }
    }
}

/// How a thread's work ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended<T> {
    /// It finished.
    Done(T),
    /// It failed, with a message.
    Failed(String),
    /// [`Progress::cancel`] stopped it.
    Cancelled,
}

/// Work on a thread of its own.
pub struct Worker<T> {
    progress: Progress,
    finished: Receiver<Ended<T>>,
}

impl<T: Send + 'static> Worker<T> {
    pub(crate) fn start(
        name: &str,
        work: impl FnOnce(&Progress) -> Ended<T> + Send + 'static,
    ) -> Result<Self, String> {
        let progress = Progress::default();
        let (send, finished) = mpsc::channel();
        let shared = progress.clone();
        thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let _ = send.send(work(&shared));
            })
            .map_err(|e| format!("couldn't start {name}: {e}"))?;
        Ok(Self { progress, finished })
    }

    /// Its progress.
    #[must_use]
    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    /// How it ended; `None` while it's still going.
    pub fn poll(&mut self) -> Option<Ended<T>> {
        match self.finished.try_recv() {
            Ok(ended) => Some(ended),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Ended::Failed("the work stopped unexpectedly".to_owned()))
            }
        }
    }
}

/// Where the network comes from.
pub enum Backend {
    /// ONNX Runtime's library and the model, from the Packages manager.
    Packages {
        /// ONNX Runtime's library.
        runtime: PathBuf,
        /// The model.
        model: PathBuf,
        /// Where Core ML keeps its compiled model (R34).
        cache: PathBuf,
    },
    /// A generated stand-in for tests, which splits a mix into four fixed
    /// shares that sum to it.
    Fixture,
}

/// One piece of audio to separate.
pub enum Input {
    /// A region of a file of the set's media.
    Region {
        /// The file.
        source: ContentHash,
        /// Where in the file to start and stop, in seconds.
        region: (f64, f64),
    },
    /// Audio already rendered, planar stereo at 44.1 kHz.
    Frames([Vec<f32>; 2]),
}

/// What a separation works on.
pub struct Job {
    /// The network.
    pub backend: Backend,
    /// The set's media, which decodes the files.
    pub media: Arc<MediaStore>,
    /// What to separate, one after another.
    pub inputs: Vec<Input>,
    /// How hard to work.
    pub quality: Quality,
}

/// A separation's result: the stems of the region, at 44.1 kHz, and where
/// the region starts in the file, in frames at that rate.
pub struct Separated {
    /// The region's first frame.
    pub from: usize,
    /// The stems.
    pub stems: separation::Stems,
}

/// A stand-in for the network that splits a window into fixed shares of
/// itself, Drums, Bass, Others and Vocals in that order, which sum to 1.
struct Shares;

impl separation::Network for Shares {
    fn window(&self) -> usize {
        4_096
    }

    fn run(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
        Ok([0.2f32, 0.3, 0.4, 0.1]
            .iter()
            .flat_map(|share| input.iter().map(move |sample| sample * share))
            .collect())
    }
}

/// Starts separating on a thread of its own.
///
/// # Errors
///
/// A message if the thread can't start.
pub fn separate(job: Job) -> Result<Worker<Vec<Separated>>, String> {
    Worker::start("bl-shell-stems", move |progress| {
        match run_separation(&job, progress) {
            Ok(separated) => Ended::Done(separated),
            Err(separation::Error::Cancelled) => Ended::Cancelled,
            Err(separation::Error::Failed(error)) => Ended::Failed(error),
        }
    })
}

fn run_separation(job: &Job, progress: &Progress) -> Result<Vec<Separated>, separation::Error> {
    let failed = separation::Error::Failed;
    progress.total.store(1000, Ordering::Relaxed);
    progress.set_stage("Loading the model");
    let mut demucs = match &job.backend {
        Backend::Packages {
            runtime,
            model,
            cache,
        } => {
            let threads = thread::available_parallelism().map_or(4, std::num::NonZero::get);
            let options = separation::onnx::Options {
                threads,
                accelerate: true,
                cache: Some(cache.clone()),
            };
            let network = separation::onnx::Demucs::load(runtime, model, &options, &mut |stage| {
                progress.set_stage(stage);
            })
            .map_err(failed)?;
            progress.set_provider(network.report().describe());
            Some(network)
        }
        Backend::Fixture => None,
    };
    let mut shares = Shares;
    let count = job.inputs.len();
    let mut out = Vec::with_capacity(count);
    for (index, input) in job.inputs.iter().enumerate() {
        progress.set_stage("Reading the audio");
        let (from, [left, right]) = match input {
            Input::Region { source, region } => region_frames(job, *source, *region)?,
            Input::Frames(frames) => (0, frames.clone()),
        };
        if progress.cancelled() {
            return Err(separation::Error::Cancelled);
        }
        progress.set_stage(match &demucs {
            Some(network) => format!("Separating on {}", network.report().describe()),
            None => "Separating".to_owned(),
        });
        // Each input's share of the bar, so several tracks fill it in turn.
        let share = |fraction: f32| (index as f32 + fraction) / count as f32;
        let stems = separation::separate(
            &left,
            &right,
            match &mut demucs {
                Some(network) => network,
                None => &mut shares,
            },
            job.quality,
            &mut |fraction| {
                progress.done.store(
                    (f64::from(share(fraction)) * 1000.0) as u64,
                    Ordering::Relaxed,
                );
                !progress.cancelled()
            },
        );
        // A GPU run that failed part way finished on the CPU (R33).
        if let Some(network) = &demucs {
            progress.set_provider(network.report().describe());
        }
        out.push(Separated {
            from,
            stems: stems?,
        });
    }
    Ok(out)
}

/// A file's region at 44.1 kHz as planar stereo, and where it starts in the
/// file, in frames.
fn region_frames(
    job: &Job,
    source: ContentHash,
    region: (f64, f64),
) -> Result<(usize, [Vec<f32>; 2]), separation::Error> {
    let failed = separation::Error::Failed;
    let audio = job
        .media
        .load(source)
        .map_err(|e| failed(format!("couldn't read the clip's file: {e}")))?;
    let audio = if audio.sample_rate() == RATE {
        audio
    } else {
        Arc::new(
            bl_media::convert_sample_rate(&audio, RATE)
                .map_err(|e| failed(format!("couldn't convert the clip's file: {e}")))?,
        )
    };
    let frames = audio.frames();
    let at = |seconds: f64| ((seconds * f64::from(RATE)).round().max(0.0) as usize).min(frames);
    let (from, to) = (at(region.0), at(region.1));
    if to <= from {
        return Err(failed("the clip plays no audio to separate".to_owned()));
    }
    let left = audio
        .channel(0)
        .ok_or_else(|| failed("the clip's file has no audio".to_owned()))?;
    let right = audio.channel(1).unwrap_or(left);
    Ok((from, [left[from..to].to_vec(), right[from..to].to_vec()]))
}

/// The frames of one file the separation writes: `from` frames of silence,
/// so the file lines up with the clip's own, then the stems in `parts`
/// summed, interleaved stereo.
#[must_use]
pub fn file_frames(separated: &Separated, parts: &[Stem]) -> Vec<f32> {
    let length = separated.stems.stems[0][0].len();
    let mut out = vec![0.0f32; 2 * (separated.from + length)];
    for stem in parts {
        let [left, right] = separated.stems.get(*stem);
        for i in 0..length {
            let at = 2 * (separated.from + i);
            out[at] += left[i];
            out[at + 1] += right[i];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stems_file_lines_up_with_the_clip_and_sums_its_parts() {
        let stems = separation::Stems {
            stems: std::array::from_fn(|i| [vec![i as f32; 3], vec![-(i as f32); 3]]),
        };
        let separated = Separated { from: 2, stems };
        let frames = file_frames(&separated, &[Stem::Bass, Stem::Vocals]);
        assert_eq!(frames.len(), 2 * 5);
        assert_eq!(&frames[..4], &[0.0; 4]);
        // Bass is 1, Vocals 3.
        assert_eq!(&frames[4..6], &[4.0, -4.0]);
    }
}
