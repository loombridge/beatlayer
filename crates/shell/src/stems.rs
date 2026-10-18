//! Stem separation's background work (WP-9.1): the packages it downloads on
//! first use, and the separation itself, each on a thread of its own. The
//! shell starts them and lands their results (`Shell::run` for `stems.*`);
//! the behaviour is in the stem separation spec.
//!
//! The packages are ONNX Runtime's shared library, for this platform, from
//! its GitHub release, and HTDemucs exported to ONNX. Each download is
//! checked against its SHA-256 before anything is installed, and installed
//! files only appear once whole, so a folder holds a package or nothing.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;

use bl_analysis::stems::{self as separation, Quality, RATE, Stem};
use bl_media::{ContentHash, MediaStore};
use sha2::{Digest, Sha256};

/// ONNX Runtime's version: the one `bl-analysis` is built against.
pub const RUNTIME_VERSION: &str = "1.28.3";

/// How a download becomes installed files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unpack {
    /// The download is the file.
    File,
    /// A gzipped tar archive holding the library.
    TarGz,
    /// A zip archive holding the library.
    Zip,
}

/// A package stem separation needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    /// Its name.
    pub name: &'static str,
    /// What it does, for the dialog.
    pub purpose: &'static str,
    /// Its version.
    pub version: &'static str,
    /// Its project's page.
    pub project: &'static str,
    /// What's downloaded.
    pub download: &'static str,
    /// The download's size in bytes.
    pub size: u64,
    /// The download's SHA-256, in hex.
    pub sha256: &'static str,
    /// How it's installed.
    pub unpack: Unpack,
    /// Its folder in the packages folder.
    pub folder: &'static str,
    /// The file that's there once it's installed, in its folder.
    pub file: &'static str,
}

impl Package {
    /// Where its main file is installed under `packages`.
    #[must_use]
    pub fn path(&self, packages: &Path) -> PathBuf {
        packages.join(self.folder).join(self.file)
    }

    /// Whether it's installed under `packages`.
    #[must_use]
    pub fn installed(&self, packages: &Path) -> bool {
        std::fs::metadata(self.path(packages)).is_ok_and(|m| m.is_file() && m.len() > 0)
    }
}

/// ONNX Runtime for this platform, if it has one.
#[must_use]
pub fn runtime() -> Option<Package> {
    let (download, size, sha256, unpack, file) = if cfg!(all(
        target_os = "macos",
        target_arch = "aarch64"
    )) {
        (
            "https://github.com/microsoft/onnxruntime/releases/download/v1.28.3/onnxruntime-osx-arm64-1.28.3.tgz",
            33_057_743,
            "c436bd9f47dbce6f6311ccc21de97f829b3c862c09d24aa321958cb72f0a1d32",
            Unpack::TarGz,
            "libonnxruntime.dylib",
        )
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        (
            "https://github.com/microsoft/onnxruntime/releases/download/v1.28.3/onnxruntime-win-x64-1.28.3.zip",
            78_606_946,
            "1d6fab48e85f948436af7c8c971d2c145cf224e2c444755dec894f8b0de11a83",
            Unpack::Zip,
            "onnxruntime.dll",
        )
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        (
            "https://github.com/microsoft/onnxruntime/releases/download/v1.28.3/onnxruntime-linux-x64-1.28.3.tgz",
            9_130_098,
            "db14e4863bd37893fc59729d986ab2a0d043d10b7d44da1913c4982b7e3d009c",
            Unpack::TarGz,
            "libonnxruntime.so",
        )
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        (
            "https://github.com/microsoft/onnxruntime/releases/download/v1.28.3/onnxruntime-linux-aarch64-1.28.3.tgz",
            8_123_797,
            "6c6b1ae96d7b0be9f555092857c6a4f2b0b5587a5298d732d9e52e8253038ce7",
            Unpack::TarGz,
            "libonnxruntime.so",
        )
    } else {
        return None;
    };
    Some(Package {
        name: "ONNX Runtime",
        purpose: "Runs the separation model on this computer",
        version: RUNTIME_VERSION,
        project: "https://github.com/microsoft/onnxruntime",
        download,
        size,
        sha256,
        unpack,
        folder: "onnxruntime-1.28.3",
        file,
    })
}

/// HTDemucs, exported to ONNX.
#[must_use]
pub fn model() -> Package {
    Package {
        name: "Demucs (HTDemucs)",
        purpose: "The model that separates vocals, drums, bass and others",
        version: "4.1.0",
        project: "https://github.com/adefossez/demucs",
        download: "https://huggingface.co/StemSplitio/htdemucs-onnx/resolve/main/htdemucs.onnx",
        size: 316_446_953,
        sha256: "68d0bf16428ef66e692cdff8a9ccf28f1ef3f69440d57e58605a4cc55fcc5e74",
        unpack: Unpack::File,
        folder: "demucs-4.1.0",
        file: "htdemucs.onnx",
    }
}

/// Every package, in the order they're downloaded; empty on a platform
/// ONNX Runtime doesn't publish a library for.
#[must_use]
pub fn packages() -> Vec<Package> {
    runtime().map_or_else(Vec::new, |runtime| vec![runtime, model()])
}

/// What a download or separation is doing now, shared with its thread.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    stage: Arc<Mutex<String>>,
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

    /// Asks the thread to stop.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn set_stage(&self, stage: impl Into<String>) {
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
    fn start(
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

/// Starts downloading and installing the packages not yet in `folder`.
///
/// # Errors
///
/// A message if the thread can't start.
pub fn install(folder: PathBuf) -> Result<Worker<()>, String> {
    Worker::start("bl-shell-stems-install", move |progress| {
        let wanted: Vec<Package> = packages()
            .into_iter()
            .filter(|p| !p.installed(&folder))
            .collect();
        progress
            .total
            .store(wanted.iter().map(|p| p.size).sum(), Ordering::Relaxed);
        for package in &wanted {
            match install_one(package, &folder, progress) {
                Ok(()) => {}
                Err(_) if progress.cancelled() => return Ended::Cancelled,
                Err(error) => return Ended::Failed(error),
            }
        }
        Ended::Done(())
    })
}

/// Downloads one package, checks it, and installs it.
fn install_one(package: &Package, folder: &Path, progress: &Progress) -> Result<(), String> {
    let destination = folder.join(package.folder);
    std::fs::create_dir_all(&destination)
        .map_err(|e| format!("couldn't make {}: {e}", destination.display()))?;
    progress.set_stage(format!("Downloading {}", package.name));
    let part = destination.join(".download");
    let result = download(package, &part, progress).and_then(|()| {
        progress.set_stage(format!("Installing {}", package.name));
        unpack(package, &part, &destination)
    });
    let _ = std::fs::remove_file(&part);
    result
}

/// Downloads `package` to `part`, checking its size and SHA-256.
fn download(package: &Package, part: &Path, progress: &Progress) -> Result<(), String> {
    let failed = |e: &dyn std::fmt::Display| format!("couldn't download {}: {e}", package.name);
    let response = bl_assistant::provider::agent()
        .get(package.download)
        .call()
        .map_err(|e| failed(&e))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(failed(&format!("the server answered {status}")));
    }
    let mut body = response.into_body().into_reader();
    let mut file = std::fs::File::create(part).map_err(|e| failed(&e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut received = 0u64;
    loop {
        if progress.cancelled() {
            return Err("cancelled".to_owned());
        }
        let n = body.read(&mut buffer).map_err(|e| failed(&e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        file.write_all(&buffer[..n]).map_err(|e| failed(&e))?;
        received += n as u64;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.sync_all().map_err(|e| failed(&e))?;
    if received != package.size {
        return Err(failed(&format!(
            "it was {received} bytes, not the {} expected",
            package.size
        )));
    }
    let digest = hex(&hasher.finalize());
    if digest != package.sha256 {
        return Err(failed(&"its SHA-256 doesn't match the one expected"));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Installs a checked download into `destination`: the file itself, or the
/// library and its licence notices from an archive. The main file is
/// written last, under a temporary name, so the package shows installed
/// only once it's whole.
fn unpack(package: &Package, part: &Path, destination: &Path) -> Result<(), String> {
    let failed = |e: &dyn std::fmt::Display| format!("couldn't install {}: {e}", package.name);
    let target = destination.join(package.file);
    match package.unpack {
        Unpack::File => std::fs::rename(part, &target).map_err(|e| failed(&e)),
        Unpack::TarGz => {
            let file = std::fs::File::open(part).map_err(|e| failed(&e))?;
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
            let mut found = false;
            for entry in archive.entries().map_err(|e| failed(&e))? {
                let mut entry = entry.map_err(|e| failed(&e))?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                let path = entry.path().map_err(|e| failed(&e))?.into_owned();
                let Some(name) = wanted(package, &path) else {
                    continue;
                };
                found |= name == package.file;
                write_atomically(&destination.join(name), &mut entry).map_err(|e| failed(&e))?;
            }
            found
                .then_some(())
                .ok_or_else(|| failed(&"the archive holds no library"))
        }
        Unpack::Zip => {
            let file = std::fs::File::open(part).map_err(|e| failed(&e))?;
            let mut archive = zip::ZipArchive::new(file).map_err(|e| failed(&e))?;
            let mut found = false;
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).map_err(|e| failed(&e))?;
                if !entry.is_file() {
                    continue;
                }
                let path = PathBuf::from(entry.name().map_err(|e| failed(&e))?.into_owned());
                let Some(name) = wanted(package, &path) else {
                    continue;
                };
                found |= name == package.file;
                write_atomically(&destination.join(name), &mut entry).map_err(|e| failed(&e))?;
            }
            found
                .then_some(())
                .ok_or_else(|| failed(&"the archive holds no library"))
        }
    }
}

/// The name an archive's file is installed under, if it's one to keep: the
/// shared library (`lib/libonnxruntime.1.28.3.dylib` and the like, under
/// the package's own file name) and the licence notices.
fn wanted(package: &Package, path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    let in_lib = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|d| d == "lib");
    let library = match package.file {
        "libonnxruntime.dylib" => name == format!("libonnxruntime.{RUNTIME_VERSION}.dylib"),
        "libonnxruntime.so" => name == format!("libonnxruntime.so.{RUNTIME_VERSION}"),
        "onnxruntime.dll" => name == "onnxruntime.dll",
        _ => false,
    };
    if library && in_lib {
        return Some(package.file);
    }
    match name {
        "LICENSE" => Some("LICENSE"),
        "ThirdPartyNotices.txt" => Some("ThirdPartyNotices.txt"),
        _ => None,
    }
}

/// Writes `from` to `to` through a temporary file beside it.
fn write_atomically(to: &Path, from: &mut dyn Read) -> std::io::Result<()> {
    let part = to.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    std::io::copy(from, &mut file)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&part, to)
}

/// What a separation works on.
pub struct Job {
    /// The packages' folder.
    pub packages: PathBuf,
    /// The set's media, which decodes the clip's file.
    pub media: Arc<MediaStore>,
    /// The clip's file.
    pub source: ContentHash,
    /// Where in the file to start and stop, in seconds.
    pub region: (f64, f64),
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

/// Starts separating on a thread of its own.
///
/// # Errors
///
/// A message if the thread can't start.
pub fn separate(job: Job) -> Result<Worker<Separated>, String> {
    Worker::start("bl-shell-stems", move |progress| {
        match run_separation(&job, progress) {
            Ok(separated) => Ended::Done(separated),
            Err(separation::Error::Cancelled) => Ended::Cancelled,
            Err(separation::Error::Failed(error)) => Ended::Failed(error),
        }
    })
}

fn run_separation(job: &Job, progress: &Progress) -> Result<Separated, separation::Error> {
    let failed = separation::Error::Failed;
    let (Some(runtime), model) = (runtime(), model()) else {
        return Err(failed(
            "stem separation isn't available on this platform".to_owned(),
        ));
    };
    progress.set_stage("Reading the clip");
    progress.total.store(1000, Ordering::Relaxed);
    let audio = job
        .media
        .load(job.source)
        .map_err(|e| failed(format!("couldn't read the clip's file: {e}")))?;
    let audio = if audio.sample_rate() == RATE {
        audio
    } else {
        Arc::new(
            bl_media::convert_sample_rate(&audio, RATE)
                .map_err(|e| failed(format!("couldn't convert the clip's file: {e}")))?,
        )
    };
    if progress.cancelled() {
        return Err(separation::Error::Cancelled);
    }
    let frames = audio.frames();
    let at = |seconds: f64| ((seconds * f64::from(RATE)).round().max(0.0) as usize).min(frames);
    let (from, to) = (at(job.region.0), at(job.region.1));
    if to <= from {
        return Err(failed("the clip plays no audio to separate".to_owned()));
    }
    let left = audio
        .channel(0)
        .ok_or_else(|| failed("the clip's file has no audio".to_owned()))?;
    let right = audio.channel(1).unwrap_or(left);
    progress.set_stage("Loading the model");
    let threads = thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let mut network = separation::onnx::Demucs::load(
        &runtime.path(&job.packages),
        &model.path(&job.packages),
        threads,
    )
    .map_err(failed)?;
    progress.set_stage("Separating");
    let stems = separation::separate(
        &left[from..to],
        &right[from..to],
        &mut network,
        job.quality,
        &mut |fraction| {
            progress
                .done
                .store((f64::from(fraction) * 1000.0) as u64, Ordering::Relaxed);
            !progress.cancelled()
        },
    )?;
    Ok(Separated { from, stems })
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
    fn this_platform_has_its_packages() {
        if cfg!(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
        )) {
            let packages = packages();
            assert_eq!(packages.len(), 2);
            assert!(packages.iter().all(|p| p.size > 0 && p.sha256.len() == 64));
            assert!(
                packages
                    .iter()
                    .all(|p| p.project.starts_with("https://github.com/"))
            );
        }
    }

    #[test]
    fn only_the_library_and_its_notices_are_kept_from_the_archive() {
        let Some(runtime) = runtime() else { return };
        let library = match runtime.file {
            "libonnxruntime.dylib" => format!(
                "onnxruntime-osx-arm64-{RUNTIME_VERSION}/lib/libonnxruntime.{RUNTIME_VERSION}.dylib"
            ),
            "libonnxruntime.so" => format!(
                "onnxruntime-linux-x64-{RUNTIME_VERSION}/lib/libonnxruntime.so.{RUNTIME_VERSION}"
            ),
            _ => format!("onnxruntime-win-x64-{RUNTIME_VERSION}/lib/onnxruntime.dll"),
        };
        assert_eq!(wanted(&runtime, Path::new(&library)), Some(runtime.file));
        assert_eq!(wanted(&runtime, Path::new("x/LICENSE")), Some("LICENSE"));
        assert_eq!(
            wanted(&runtime, Path::new("x/include/onnxruntime_c_api.h")),
            None
        );
        assert_eq!(
            wanted(
                &runtime,
                Path::new(
                    "x/lib/libonnxruntime.1.28.3.dylib.dSYM/Contents/Resources/DWARF/libonnxruntime.1.28.3.dylib"
                )
            ),
            None
        );
    }

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
