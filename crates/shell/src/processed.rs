//! Processed audio files (WP-4.20): the one writer for the files an edit
//! makes, so Reverse, Crop Sample, Consolidate, Freeze, Bounce, the sampler
//! devices, the autosampler and stem separation call it and write nothing of their own. The
//! behaviour it follows is in the audio clip editing spec, R1 to R6.
//!
//! A caller hands [`Job::start`] a [`Request`]: the kind of file, the name it
//! wants, the format, and a closure that feeds frames into a [`Sink`], from a
//! decode (Reverse, Crop) or a render (Consolidate, Freeze, Bounce). The
//! frames are written on a worker thread, as 32-bit float WAV, into a hidden
//! file beside the destination. Nothing on the audio thread waits on it.
//!
//! When the file is whole, [`Job::poll`] gives it back as a [`Part`] and
//! [`Part::place`] puts it under its name on the main thread, where a free
//! name is chosen (`Name.wav`, `Name 2.wav`, ...) or, if the set already has
//! media with the same bytes, the new file is dropped and that media is
//! used. A failure deletes what was written. [`Shell::start_processed`]
//! (`crate::Shell`) wraps all this with the Status Bar and the caller's edit.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use bl_media::{AudioWriter, BitDepth, ContentHash, EncodeSpec, FileType};

use crate::files::free_import;

/// The kind of a processed file, which names its folder under
/// `Samples/Processed/` (R1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A reversed copy of a clip's file.
    Reverse,
    /// A clip's region, cropped out.
    Crop,
    /// A range rendered by Consolidate.
    Consolidate,
    /// A frozen track's render.
    Freeze,
    /// A bounced track's render.
    Bounce,
    /// A clip's separated stems (IMP-11).
    Stems,
}

impl Kind {
    /// The folder's name.
    #[must_use]
    pub const fn folder(self) -> &'static str {
        match self {
            Self::Reverse => "Reverse",
            Self::Crop => "Crop",
            Self::Consolidate => "Consolidate",
            Self::Freeze => "Freeze",
            Self::Bounce => "Bounce",
            Self::Stems => "Stems",
        }
    }
}

/// Where a request's closure sends its frames.
pub struct Sink<'a> {
    writer: &'a mut AudioWriter,
    done: &'a AtomicU64,
}

impl Sink<'_> {
    /// Writes whole frames of interleaved samples.
    ///
    /// # Errors
    ///
    /// A message if the file can't be written, a full disk say. The closure
    /// should return it, so the job deletes the file.
    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        self.writer.write(samples).map_err(|e| e.to_string())?;
        self.done.store(self.writer.frames(), Ordering::Relaxed);
        Ok(())
    }
}

/// What feeds a file: sends the frames to the sink, on the worker thread.
pub type Produce = Box<dyn FnOnce(&mut Sink<'_>) -> Result<(), String> + Send>;

/// What to write.
pub struct Request {
    /// Which folder the file goes in.
    pub kind: Kind,
    /// The name it would like, with its `.wav` extension (R3).
    pub name: String,
    /// The channels and rate: the source's own for a copy, the engine's for
    /// a render (R2). It must be 32-bit float WAV.
    pub spec: EncodeSpec,
    /// How many frames the closure will write, for the progress; 0 if it
    /// isn't known.
    pub frames: u64,
    /// Sends the frames to the sink, on the worker thread.
    pub produce: Produce,
}

/// A file the worker has written whole, hidden beside where it goes.
#[derive(Debug)]
pub struct Part {
    folder: PathBuf,
    path: PathBuf,
    name: String,
    /// The content hash of the file's bytes.
    pub hash: ContentHash,
    /// Its size in bytes.
    pub size: u64,
}

/// A processed file in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    /// Where it is.
    pub path: PathBuf,
    /// The content hash of its bytes.
    pub hash: ContentHash,
    /// Its size in bytes.
    pub size: u64,
}

impl Part {
    /// Puts the file under a free name in its folder (R3). If `existing`
    /// is the path of media the set already has with the same bytes, the
    /// new file is deleted and that media is returned instead.
    ///
    /// # Errors
    ///
    /// A message if no name is free or the file can't be moved. The hidden
    /// file is deleted.
    pub fn place(self, existing: Option<PathBuf>) -> Result<Written, String> {
        let Self {
            folder,
            path,
            name,
            hash,
            size,
        } = self;
        let result = match existing {
            Some(path) => Ok(path),
            None => free_import(&folder, &name).and_then(|destination| {
                std::fs::rename(&path, &destination)
                    .map(|()| destination)
                    .map_err(|e| format!("couldn't move {} into place: {e}", path.display()))
            }),
        };
        // After a move this finds nothing to delete.
        let _ = std::fs::remove_file(&path);
        result.map(|path| Written { path, hash, size })
    }
}

/// A file being written on a worker thread.
pub struct Job {
    part: PathBuf,
    name: String,
    total: u64,
    done: Arc<AtomicU64>,
    finished: Receiver<Result<(ContentHash, u64), String>>,
    folder: PathBuf,
}

impl Job {
    /// Makes `folder` if it isn't there, and starts writing `request`'s file
    /// in it (R1, R6).
    ///
    /// # Errors
    ///
    /// A message if the format isn't 32-bit float WAV (R2), the folder can't
    /// be made, or the thread can't start.
    pub fn start(folder: &Path, request: Request) -> Result<Self, String> {
        if request.spec.file_type != FileType::Wav || request.spec.bit_depth != BitDepth::Float32 {
            return Err("processed files are WAVs of 32-bit floating-point samples".to_owned());
        }
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("couldn't make {}: {e}", folder.display()))?;
        let Request {
            name,
            spec,
            frames,
            produce,
            ..
        } = request;
        // Hidden, so a file half written never looks like media.
        let part = folder.join(format!(".writing-{}-{name}", unique()));
        let done = Arc::new(AtomicU64::new(0));
        let (send, finished) = mpsc::channel();
        let (path, counter) = (part.clone(), Arc::clone(&done));
        thread::Builder::new()
            .name("bl-shell-processed".into())
            .spawn(move || {
                let result = write(&path, spec, &counter, produce);
                // A failure, or nobody left to take the file (the set was
                // closed, the app is quitting), leaves nothing behind.
                let failed = result.is_err();
                if send.send(result).is_err() || failed {
                    let _ = std::fs::remove_file(&path);
                }
            })
            .map_err(|e| format!("couldn't start writing {name}: {e}"))?;
        Ok(Self {
            part,
            name,
            total: frames,
            done,
            finished,
            folder: folder.to_owned(),
        })
    }

    /// The name the file was asked for.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How much is written, from 0 to 1; 0 while the length isn't known.
    #[must_use]
    pub fn progress(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f64 / self.total as f64).min(1.0) as f32
    }

    /// The file, once it's whole; `None` while the worker is still going.
    ///
    /// # Errors
    ///
    /// The worker's message if it failed. What it wrote is deleted.
    pub fn poll(&mut self) -> Option<Result<Part, String>> {
        let result = match self.finished.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("the writer stopped unexpectedly".to_owned()),
        };
        Some(match result {
            Ok((hash, size)) => Ok(Part {
                folder: self.folder.clone(),
                path: self.part.clone(),
                name: self.name.clone(),
                hash,
                size,
            }),
            Err(error) => {
                let _ = std::fs::remove_file(&self.part);
                Err(error)
            }
        })
    }
}

impl Drop for Job {
    /// A file the worker finished but nobody took (the set was replaced)
    /// is deleted, as the worker deletes one it can't hand over.
    fn drop(&mut self) {
        if let Ok(Ok(_)) = self.finished.try_recv() {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}

/// A number no other file this run has, for hidden names.
fn unique() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    (u64::from(std::process::id()) << 32) | NEXT.fetch_add(1, Ordering::Relaxed)
}

fn write(
    path: &Path,
    spec: EncodeSpec,
    done: &AtomicU64,
    produce: Produce,
) -> Result<(ContentHash, u64), String> {
    let mut writer = AudioWriter::create(path, spec).map_err(|e| e.to_string())?;
    produce(&mut Sink {
        writer: &mut writer,
        done,
    })?;
    writer.finish().map_err(|e| e.to_string())?;
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    let hash = ContentHash::of_file(path).map_err(|e| e.to_string())?;
    Ok((hash, size))
}
