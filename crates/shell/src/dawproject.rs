//! DAWproject import and export through the shell (WP-6.36, WP-6.37): the
//! behaviour behind `set.import_dawproject`, `set.import_report` and
//! `set.export_dawproject`.
//!
//! The reading, the extracting and the building run on a worker thread, so
//! the shell's own thread isn't held up while a big file is read; the set
//! changes only when the new set is whole, and a failure leaves the open set
//! as it was (the DAWproject spec, R10). The export runs on a worker thread
//! too, and writes a hidden file beside the destination before it renames it
//! into place (R65).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use bl_interop::dawproject::{self, Installed, MediaFiles, NewColours, Plugins};
use bl_model::{ClipId, ContentHash, Document, Editor, ParamDescriptor, TrackId, WarpMode};
use bl_shell_spec::dawproject::{ExportDawProjectResult, ImportReport, ImportReportEntry};
use bl_shell_spec::settings::ThemeLook;

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

/// What an import takes from the shell besides the file (R23, R46).
pub struct Settings {
    /// The Default Warp Mode (R31).
    pub default_warp_mode: WarpMode,
    /// The 14 track colours of the theme in use (R23).
    pub track_colours: Vec<[u8; 3]>,
    /// The colours a new track and a new clip take (R23).
    pub look: ThemeLook,
    /// The plug-ins the scanner found (R46).
    pub installed: Vec<Installed>,
}

/// The colours a new track and clip take, as the settings give them.
struct LookColours(ThemeLook);

impl NewColours for LookColours {
    fn track(&self, id: TrackId) -> u8 {
        self.0.new_track_colour(id)
    }

    fn clip(&self, id: ClipId) -> Option<u8> {
        self.0.new_clip_colour(id)
    }
}

/// A plug-in the import wants described, and where to send what it finds.
struct Describe {
    plugin: Installed,
    reply: mpsc::Sender<Result<Vec<ParamDescriptor>, String>>,
}

/// The plug-ins as the worker sees them: the scanner's list, and the shell's
/// own thread to load one on, which VST3 needs (R57).
struct Hosted {
    installed: Vec<Installed>,
    ask: Mutex<mpsc::Sender<Describe>>,
}

impl Plugins for Hosted {
    fn installed(&self) -> &[Installed] {
        &self.installed
    }

    fn describe(&self, plugin: &Installed) -> Result<Vec<ParamDescriptor>, String> {
        let (reply, answer) = mpsc::channel();
        self.ask
            .lock()
            .map_err(|_| "the import stopped".to_owned())?
            .send(Describe {
                plugin: plugin.clone(),
                reply,
            })
            .map_err(|_| "the import stopped".to_owned())?;
        answer
            .recv()
            .unwrap_or_else(|_| Err("the shell stopped answering".to_owned()))
    }
}

/// An import running on a worker thread (R10).
pub struct Job {
    percent: Arc<AtomicU32>,
    finished: Receiver<Result<Imported, DawProjectError>>,
    asked: Receiver<Describe>,
}

impl Job {
    /// Starts reading the DAWproject file at `path` and making the set it
    /// holds, writing the files it holds into `folder`.
    ///
    /// # Errors
    ///
    /// [`DawProjectError`] if the thread can't start.
    pub fn start(path: &Path, folder: &Path, settings: Settings) -> Result<Self, DawProjectError> {
        let (send, finished) = mpsc::channel();
        let (ask, asked) = mpsc::channel();
        let percent = Arc::new(AtomicU32::new(0));
        let (path, folder, counter) = (path.to_path_buf(), folder.to_path_buf(), percent.clone());
        std::thread::Builder::new()
            .name("bl-shell-dawproject".into())
            .spawn(move || {
                let plugins = Hosted {
                    installed: settings.installed,
                    ask: Mutex::new(ask),
                };
                let colours = LookColours(settings.look);
                let _ = send.send(build(
                    &path,
                    &folder,
                    settings.default_warp_mode,
                    &settings.track_colours,
                    &colours,
                    &plugins,
                    &counter,
                ));
            })
            .map_err(|error| {
                DawProjectError::Failed(format!("couldn't start reading the file: {error}"))
            })?;
        Ok(Self {
            percent,
            finished,
            asked,
        })
    }

    /// Answers the plug-ins the worker has asked to have loaded, with
    /// `describe` (R57). Called on the thread that owns the plug-in hosts.
    pub fn serve(
        &self,
        mut describe: impl FnMut(&Installed) -> Result<Vec<ParamDescriptor>, String>,
    ) {
        while let Ok(Describe { plugin, reply }) = self.asked.try_recv() {
            let _ = reply.send(describe(&plugin));
        }
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
    track_colours: &[[u8; 3]],
    new_colours: &dyn NewColours,
    plugins: &dyn Plugins,
    percent: &AtomicU32,
) -> Result<Imported, DawProjectError> {
    let failed = |reason: String| DawProjectError::Failed(reason);
    // The file is read from disk as the import asks for it, never all at once.
    let file = File::open(path).map_err(|error| failed(format!("{}: {error}", path.display())))?;
    let container = dawproject::read_container_from(
        std::io::BufReader::new(file),
        dawproject::Limits::default(),
    )
    .map_err(|error| failed(error.to_string()))?;
    let context = dawproject::Context {
        folder: path.parent().unwrap_or(Path::new("")),
        extract_to: folder,
        default_warp_mode,
        track_colours,
        new_colours,
        plugins,
    };
    percent.store(20, Ordering::Relaxed);
    let imported =
        dawproject::import(&container, &context).map_err(|error| failed(error.to_string()))?;
    percent.store(40, Ordering::Relaxed);

    // Spec: dawproject R5. The destination has to have room for every file
    // the project uses, by the sizes the container's headers give, or the
    // import is refused and nothing is written. The import has only read
    // the files so far, through the system's temporary folder, one at a time.
    let needed: u64 = imported
        .files
        .iter()
        .filter(|file| file.on_disk.is_none())
        .map(|file| file.size)
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
    // Spec: dawproject R73. Two files whose paths a case-insensitive file
    // system holds as one are written under different names.
    let mut taken: HashSet<String> = HashSet::new();
    let extracted = (|| {
        for (index, file) in imported.files.iter().enumerate() {
            percent.store(
                40 + (60 * index / imported.files.len()) as u32,
                Ordering::Relaxed,
            );
            let at = if let Some(on_disk) = &file.on_disk {
                on_disk.clone()
            } else {
                let at = folder.join(free_path(&mut taken, &file.path));
                if let Some(parent) = at.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| {
                        failed(format!("couldn't make {}: {error}", parent.display()))
                    })?;
                }
                let mut out = File::create(&at)
                    .map_err(|error| failed(format!("{}: {error}", at.display())))?;
                written.push(at.clone());
                // A medium is unpacked from the container straight into its
                // file; only a preset's small sample is in memory.
                match &file.bytes {
                    Some(bytes) => out.write_all(bytes),
                    None => container.copy_to(&file.path, &mut out).map(|_| ()),
                }
                .map_err(|error| failed(format!("{}: {error}", at.display())))?;
                at
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

/// `path`, or `path` with " 2", " 3" and so on before its extension when a
/// file of `taken` has the same name but for its case.
fn free_path(taken: &mut HashSet<String>, path: &str) -> String {
    let dot = path
        .rfind('.')
        .filter(|dot| *dot > 0 && !path[*dot..].contains('/'));
    let (stem, extension) = dot.map_or((path, ""), |dot| path.split_at(dot));
    let mut candidate = path.to_owned();
    let mut number = 2;
    while !taken.insert(dawproject::folded(&candidate)) {
        candidate = format!("{stem} {number}{extension}");
        number += 1;
    }
    candidate
}

/// What an export takes from the shell besides the file's path (R65, R73).
pub struct ExportSettings {
    /// The set, with the plug-ins' states taken into it.
    pub document: Document,
    /// The set's name, which `metadata.xml` holds as its title (R79).
    pub title: String,
    /// The 14 track colours of the theme in use (R23).
    pub track_colours: Vec<[u8; 3]>,
    /// The plug-ins the scanner found (R77).
    pub installed: Vec<Installed>,
    /// Where each medium the set uses is.
    pub media: HashMap<ContentHash, PathBuf>,
    /// Whether the media go into the file (R73).
    pub embed_media: bool,
}

/// The media the clips and the devices of a set play.
#[must_use]
pub fn media_used(document: &Document) -> BTreeSet<ContentHash> {
    let mut used = BTreeSet::new();
    for track in document.tracks() {
        for clip in track.all_clips() {
            if let bl_model::ClipData::Audio(audio) = &clip.data {
                used.insert(audio.media);
            }
        }
        for device in track.devices() {
            used.extend(device.media_used());
        }
    }
    used
}

/// The media of a set, where they are.
struct Files(HashMap<ContentHash, PathBuf>);

impl MediaFiles for Files {
    fn path(&self, hash: ContentHash) -> Option<PathBuf> {
        self.0.get(&hash).filter(|path| path.is_file()).cloned()
    }
}

/// An export running on a worker thread (R65).
pub struct ExportJob {
    percent: Arc<AtomicU32>,
    finished: Receiver<Result<ExportDawProjectResult, DawProjectError>>,
    asked: Receiver<Describe>,
}

impl ExportJob {
    /// Starts writing the set as a DAWproject file at `path`.
    ///
    /// # Errors
    ///
    /// [`DawProjectError`] if the thread can't start.
    pub fn start(path: &Path, settings: ExportSettings) -> Result<Self, DawProjectError> {
        let (send, finished) = mpsc::channel();
        let (ask, asked) = mpsc::channel();
        let percent = Arc::new(AtomicU32::new(0));
        let (path, counter) = (path.to_path_buf(), percent.clone());
        std::thread::Builder::new()
            .name("bl-shell-dawproject-export".into())
            .spawn(move || {
                let plugins = Hosted {
                    installed: settings.installed.clone(),
                    ask: Mutex::new(ask),
                };
                let _ = send.send(write(&path, &settings, &plugins, &counter));
            })
            .map_err(|error| {
                DawProjectError::Failed(format!("couldn't start writing the file: {error}"))
            })?;
        Ok(Self {
            percent,
            finished,
            asked,
        })
    }

    /// Answers the plug-ins the worker has asked to have loaded (R76).
    pub fn serve(
        &self,
        mut describe: impl FnMut(&Installed) -> Result<Vec<ParamDescriptor>, String>,
    ) {
        while let Ok(Describe { plugin, reply }) = self.asked.try_recv() {
            let _ = reply.send(describe(&plugin));
        }
    }

    /// How far the export is, from 0 to 100, for the Status Bar.
    #[must_use]
    pub fn percent(&self) -> u32 {
        self.percent.load(Ordering::Relaxed)
    }

    /// The result, once the worker is done; `None` while it is still going.
    pub fn poll(&mut self) -> Option<Result<ExportDawProjectResult, DawProjectError>> {
        match self.finished.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(DawProjectError::Failed(
                "the writer stopped unexpectedly".to_owned(),
            ))),
        }
    }
}

/// Writes a file to `path` whole or not at all (R65): `make` writes it into a
/// hidden file beside `path`, which is renamed into place when it is whole,
/// so `path` holds the old file or the new one whenever the write stops.
fn write_whole<T>(
    path: &Path,
    make: impl FnOnce(BufWriter<File>) -> Result<(BufWriter<File>, T), dawproject::Error>,
) -> Result<T, String> {
    let io = |error: std::io::Error| format!("{}: {error}", path.display());
    let folder = path
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(folder).map_err(io)?;
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let hidden = folder.join(format!(".{name}.saving"));
    let written = (|| {
        let file = File::create(&hidden).map_err(io)?;
        let (mut file, made) = make(BufWriter::new(file)).map_err(|error| error.to_string())?;
        file.flush().map_err(io)?;
        file.get_ref().sync_all().map_err(io)?;
        drop(file);
        std::fs::rename(&hidden, path).map_err(io)?;
        Ok(made)
    })();
    if written.is_err() {
        std::fs::remove_file(&hidden).ok();
    }
    written
}

/// Writes the set as a file (R65) and says what the file leaves out.
fn write(
    path: &Path,
    settings: &ExportSettings,
    plugins: &dyn Plugins,
    percent: &AtomicU32,
) -> Result<ExportDawProjectResult, DawProjectError> {
    let media = Files(settings.media.clone());
    let context = dawproject::ExportContext {
        application_version: env!("CARGO_PKG_VERSION"),
        title: &settings.title,
        track_colours: &settings.track_colours,
        embed_media: settings.embed_media,
        media: &media,
        plugins,
    };
    percent.store(10, Ordering::Relaxed);
    let report = write_whole(path, |file| {
        dawproject::export_to(&settings.document, &context, file)
    })
    .map_err(DawProjectError::Failed)?;
    percent.store(100, Ordering::Relaxed);
    Ok(ExportDawProjectResult {
        path: path.to_string_lossy().into_owned(),
        report: ImportReport {
            application: report.application.clone(),
            entries: report
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

/// How much room the folder's filesystem has, when the platform says: the
/// room of the nearest folder that exists, since the import makes the rest.
#[cfg(unix)]
fn free_space(folder: &Path) -> Option<u64> {
    let existing = folder.ancestors().find(|path| path.exists())?;
    let stats = rustix::fs::statvfs(existing).ok()?;
    Some(stats.f_bavail.saturating_mul(stats.f_frsize))
}

/// How much room the folder's filesystem has, when the platform says.
#[cfg(not(unix))]
fn free_space(_folder: &Path) -> Option<u64> {
    None
}
