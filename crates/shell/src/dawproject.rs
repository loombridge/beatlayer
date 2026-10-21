//! DAWproject import through the shell (WP-6.36): the behaviour behind
//! `set.import_dawproject` and `set.import_report`.
//!
//! The reading, the extracting and the building run on a worker thread, so
//! the shell's own thread isn't held up while a big file is read; the set
//! changes only when the new set is whole, and a failure leaves the open set
//! as it was (the DAWproject spec, R10).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use bl_interop::dawproject;
use bl_model::{ContentHash, Editor, WarpMode};
use bl_shell_spec::dawproject::{ImportReport, ImportReportEntry};

/// Why a DAWproject command couldn't run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DawProjectError {
    /// The arguments don't fit.
    InvalidArgs(String),
    /// It ran and failed.
    Failed(String),
}

impl std::fmt::Display for DawProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidArgs(reason) => f.write_str(reason),
            Self::Failed(reason) => f.write_str(reason),
        }
    }
}

/// What an import made, ready for the shell to open.
pub struct Imported {
    /// The new set's editor, with an empty undo history (R11).
    pub editor: Editor,
    /// The media the set uses: its hash, where it is on disk, and its size.
    pub media: Vec<(ContentHash, PathBuf, u64)>,
    /// What the import left out or changed (R12).
    pub report: ImportReport,
}

/// The folder the container's files are written into, under the Temporary
/// Folder (R39): `Imported/<the file's name without .dawproject>/`.
#[must_use]
pub fn import_folder(temporary: &Path, source: &Path) -> PathBuf {
    let name = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported".to_owned());
    temporary.join("Imported").join(name)
}

/// An import running on a worker thread (R10).
pub struct Job {
    percent: Arc<AtomicU32>,
    finished: Receiver<Result<Imported, DawProjectError>>,
}

impl Job {
    /// Starts reading the DAWproject file at `path` and making the set it
    /// holds, writing the files it holds into `folder`.
    ///
    /// # Errors
    ///
    /// [`DawProjectError`] if the thread can't start.
    pub fn start(
        path: &Path,
        folder: &Path,
        default_warp_mode: WarpMode,
    ) -> Result<Self, DawProjectError> {
        let (send, finished) = mpsc::channel();
        let percent = Arc::new(AtomicU32::new(0));
        let (path, folder, counter) = (path.to_path_buf(), folder.to_path_buf(), percent.clone());
        std::thread::Builder::new()
            .name("bl-shell-dawproject".into())
            .spawn(move || {
                let _ = send.send(build(&path, &folder, default_warp_mode, &counter));
            })
            .map_err(|error| {
                DawProjectError::Failed(format!("couldn't start reading the file: {error}"))
            })?;
        Ok(Self { percent, finished })
    }

    /// How far the import is, from 0 to 100, for the Status Bar.
    #[must_use]
    pub fn percent(&self) -> u32 {
        self.percent.load(Ordering::Relaxed)
    }

    /// The result, once the worker is done; `None` while it is still going.
    pub fn poll(&mut self) -> Option<Result<Imported, DawProjectError>> {
        match self.finished.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(DawProjectError::Failed(
                "the reader stopped unexpectedly".to_owned(),
            ))),
        }
    }
}

/// Reads the file and builds the set, on the worker thread.
fn build(
    path: &Path,
    folder: &Path,
    default_warp_mode: WarpMode,
    percent: &AtomicU32,
) -> Result<Imported, DawProjectError> {
    let failed = |reason: String| DawProjectError::Failed(reason);
    let bytes =
        std::fs::read(path).map_err(|error| failed(format!("{}: {error}", path.display())))?;
    let container =
        dawproject::read_container(&bytes).map_err(|error| failed(error.to_string()))?;
    let context = dawproject::Context {
        folder: path.parent().unwrap_or(Path::new("")),
        extract_to: folder,
        default_warp_mode,
    };
    percent.store(20, Ordering::Relaxed);
    let imported =
        dawproject::import(&container, &context).map_err(|error| failed(error.to_string()))?;
    percent.store(40, Ordering::Relaxed);

    // Spec: dawproject R5. The destination has to have room for every file
    // the project uses, by the sizes the container's headers give, or the
    // import is refused and nothing is written.
    let needed: u64 = imported
        .files
        .iter()
        .filter_map(|file| file.bytes.as_ref().map(|bytes| bytes.len() as u64))
        .sum();
    if let Some(free) = free_space(folder)
        && free < needed
    {
        return Err(failed(format!(
            "{} has {free} bytes free, and the file needs {needed}",
            folder.display()
        )));
    }

    // The files the container holds are written into the Temporary Folder
    // (R39); a failure takes back what was written.
    let mut media = Vec::new();
    let mut written: Vec<PathBuf> = Vec::new();
    let extracted = (|| {
        for (index, file) in imported.files.iter().enumerate() {
            percent.store(
                40 + (60 * index / imported.files.len()) as u32,
                Ordering::Relaxed,
            );
            let at = match (&file.bytes, &file.on_disk) {
                (Some(bytes), _) => {
                    let at = folder.join(&file.path);
                    if let Some(parent) = at.parent() {
                        std::fs::create_dir_all(parent).map_err(|error| {
                            failed(format!("couldn't make {}: {error}", parent.display()))
                        })?;
                    }
                    std::fs::write(&at, bytes)
                        .map_err(|error| failed(format!("{}: {error}", at.display())))?;
                    written.push(at.clone());
                    at
                }
                (None, Some(on_disk)) => on_disk.clone(),
                _ => continue,
            };
            media.push((file.hash, at, file.size));
        }
        Ok(())
    })();
    if let Err(error) = extracted {
        for at in written {
            let _ = std::fs::remove_file(at);
        }
        return Err(error);
    }
    Ok(Imported {
        editor: imported.project.editor(),
        media,
        report: ImportReport {
            application: imported.report.application.clone(),
            entries: imported
                .report
                .entries
                .iter()
                .map(|entry| ImportReportEntry {
                    what: entry.what.clone(),
                    place: entry.place.clone(),
                    count: entry.count,
                    did: entry.did.clone(),
                })
                .collect(),
        },
    })
}

/// How much room the folder's filesystem has, when the platform says.
#[cfg(unix)]
fn free_space(folder: &Path) -> Option<u64> {
    let path = std::ffi::CString::new(folder.as_os_str().as_encoded_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: the path is a NUL-terminated C string, and the statistics are
    // written by the call.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    let block = u64::from(stats.f_frsize);
    Some(u64::from(stats.f_bavail) * block)
}

/// How much room the folder's filesystem has, when the platform says.
#[cfg(not(unix))]
fn free_space(_folder: &Path) -> Option<u64> {
    None
}
