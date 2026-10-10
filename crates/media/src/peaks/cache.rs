//! The peak cache: peaks built in the background, kept in memory, and saved
//! in the platform's cache folder, keyed by content hash.
//!
//! The UI asks for a hash's peaks with [`PeakCache::request`] as often as it
//! likes (every frame, for every clip in view): the first request starts a
//! build on the cache's own threads and returns at once, and later ones
//! return the peaks when they're ready. [`PeakCache::subscribe`] says when a
//! build finishes, so the UI can redraw.
//!
//! Peaks name the content they were built from, so they never go stale on
//! their own. What changes is whether a file still holds that content:
//!
//! - A build checks the file before it starts and again when it ends, so a
//!   file that changes while it's read gives no peaks.
//! - Peaks are loaded from disk only after the media store confirms the file
//!   still holds the content; a file whose content changed has its peak file
//!   deleted.
//! - [`PeakCache::check`] asks the store about a hash and drops its peaks if
//!   the file is missing or changed. The app calls it when it learns a file
//!   may have changed: when it regains focus (an external editor may have
//!   saved a sample), when a project opens, and before playing media again.
//!
//! An edited file has a new hash, so its new content's peaks are built under
//! the new hash once the app imports it again.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use super::Peaks;
use super::build::{BuildOptions, BuildProgress};
use super::file::FILE_EXTENSION;
use crate::error::MediaError;
use crate::hash::ContentHash;
use crate::store::{MediaStatus, MediaStore};

/// How a [`PeakCache`] is set up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeakCacheOptions {
    /// The folder peak files are saved in, or `None` to keep peaks in memory
    /// only, building them again in each session (as Live does with its
    /// "Create Analysis Files" setting off).
    pub dir: Option<PathBuf>,
    /// Background threads, shared by every build. Zero means one per core.
    pub threads: usize,
    /// How each build runs. Its `threads` is ignored: builds use the cache's
    /// threads.
    pub build: BuildOptions,
}

impl PeakCacheOptions {
    /// Options that save peak files in `dir`.
    #[must_use]
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
            ..Self::memory_only()
        }
    }

    /// Options that keep peaks in memory only.
    #[must_use]
    pub fn memory_only() -> Self {
        Self {
            dir: None,
            threads: 0,
            build: BuildOptions::default(),
        }
    }

    /// Options that save peak files in the platform's cache folder (see
    /// [`default_cache_dir`]), or keep them in memory if there isn't one.
    #[must_use]
    pub fn platform_default() -> Self {
        Self {
            dir: default_cache_dir(),
            ..Self::memory_only()
        }
    }
}

/// Where a hash's peaks stand.
#[derive(Clone, Debug)]
pub enum PeakStatus {
    /// The peaks, ready to draw.
    Ready(Arc<Peaks>),
    /// Being built (or read from disk).
    Pending {
        /// Frames decoded so far.
        frames_done: u64,
        /// Frames to decode, once the header's been read, if it says.
        frames_total: Option<u64>,
    },
    /// The build failed: the media is unknown, missing, changed, or can't be
    /// decoded. It isn't tried again until [`PeakCache::check`] finds the
    /// media available, or the hash is forgotten.
    Failed(Arc<MediaError>),
}

/// Something that happened to a hash's peaks, sent to subscribers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeakEvent {
    /// The peaks are ready.
    Ready(ContentHash),
    /// The build failed.
    Failed(ContentHash),
    /// The peaks were dropped because their source file is missing or
    /// changed.
    Invalidated {
        /// The media whose peaks were dropped.
        hash: ContentHash,
        /// What the store found.
        status: MediaStatus,
    },
}

/// What [`PeakCache::prune`] removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PruneReport {
    /// Peak files deleted.
    pub files_removed: usize,
    /// Their size in bytes.
    pub bytes_removed: u64,
    /// The size of the peak files left, in bytes.
    pub bytes_kept: u64,
}

/// Peaks built in the background and cached in memory and on disk, keyed by
/// content hash. Shareable between threads.
///
/// Dropping the cache cancels the builds in progress and waits for them to
/// stop, which takes at most one block of decoding each.
pub struct PeakCache {
    shared: Arc<Shared>,
    pool: rayon::ThreadPool,
}

struct Shared {
    store: Arc<MediaStore>,
    dir: Option<PathBuf>,
    build: BuildOptions,
    state: Mutex<State>,
    /// Signalled whenever a slot settles or is removed, and when the last
    /// running job ends.
    settled: Condvar,
    listeners: Mutex<Vec<Sender<PeakEvent>>>,
}

#[derive(Default)]
struct State {
    slots: HashMap<ContentHash, Slot>,
    /// Jobs started and not yet finished.
    running: usize,
}

#[derive(Clone)]
enum Slot {
    Pending(Arc<BuildProgress>),
    Ready(Arc<Peaks>),
    Failed(Arc<MediaError>),
}

impl std::fmt::Debug for PeakCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeakCache")
            .field("dir", &self.shared.dir)
            .field("slots", &self.shared.state().slots.len())
            .finish_non_exhaustive()
    }
}

impl PeakCache {
    /// A cache that builds peaks of media in `store`.
    ///
    /// # Errors
    ///
    /// Fails if the background threads can't be started.
    pub fn new(store: Arc<MediaStore>, options: PeakCacheOptions) -> Result<Self, MediaError> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(options.threads)
            .thread_name(|i| format!("bl-peaks-{i}"))
            .build()
            .map_err(|e| MediaError::Io(io::Error::other(e)))?;
        Ok(Self {
            shared: Arc::new(Shared {
                store,
                dir: options.dir,
                build: BuildOptions {
                    threads: None,
                    ..options.build
                },
                state: Mutex::new(State::default()),
                settled: Condvar::new(),
                listeners: Mutex::new(Vec::new()),
            }),
            pool,
        })
    }

    /// The folder peak files are saved in, if any.
    #[must_use]
    pub fn dir(&self) -> Option<&Path> {
        self.shared.dir.as_deref()
    }

    /// Where a hash's peak file is, or would be: `<dir>/<first two hex
    /// digits>/<hash>.blpeaks`.
    #[must_use]
    pub fn file_path(&self, hash: ContentHash) -> Option<PathBuf> {
        self.shared.file_path(hash)
    }

    /// The hash's peaks if they're ready; otherwise starts building them, if
    /// that isn't already under way or failed, and says how far it's got.
    /// Never waits.
    pub fn request(&self, hash: ContentHash) -> PeakStatus {
        let mut state = self.shared.state();
        if let Some(slot) = state.slots.get(&hash) {
            return slot.status();
        }
        let progress = self.start(&mut state, hash);
        Slot::Pending(progress).status()
    }

    /// The hash's peaks if they're ready, without starting a build.
    #[must_use]
    pub fn get(&self, hash: ContentHash) -> Option<Arc<Peaks>> {
        match self.shared.state().slots.get(&hash) {
            Some(Slot::Ready(peaks)) => Some(Arc::clone(peaks)),
            _ => None,
        }
    }

    /// Where the hash's peaks stand, without starting a build: `None` if
    /// nothing has asked for them.
    #[must_use]
    pub fn status(&self, hash: ContentHash) -> Option<PeakStatus> {
        self.shared.state().slots.get(&hash).map(Slot::status)
    }

    /// The hash's peaks, building them if needed and waiting until they're
    /// ready. For tools and tests; the UI uses [`PeakCache::request`]. Don't
    /// call it from the cache's own threads, which it would be waiting on.
    ///
    /// # Errors
    ///
    /// Returns the build's error, as [`PeakStatus::Failed`] holds it.
    pub fn wait(&self, hash: ContentHash) -> Result<Arc<Peaks>, Arc<MediaError>> {
        let mut state = self.shared.state();
        loop {
            match state.slots.get(&hash) {
                Some(Slot::Ready(peaks)) => return Ok(Arc::clone(peaks)),
                Some(Slot::Failed(error)) => return Err(Arc::clone(error)),
                Some(Slot::Pending(_)) => {
                    state = self
                        .shared
                        .settled
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                None => {
                    self.start(&mut state, hash);
                }
            }
        }
    }

    /// Checks that the hash's source file still holds its content, through
    /// the media store, and drops the hash's peaks if not: from memory, and,
    /// if the file changed, from disk. A hash whose build failed may be built
    /// again once its media is available.
    ///
    /// Cheap when the file's length and modification time are as they were
    /// when the store last hashed it; otherwise the store hashes it again.
    ///
    /// # Errors
    ///
    /// As for [`MediaStore::check`].
    pub fn check(&self, hash: ContentHash) -> Result<MediaStatus, MediaError> {
        let status = self.shared.store.check(hash)?;
        if status == MediaStatus::Available {
            let mut state = self.shared.state();
            if matches!(state.slots.get(&hash), Some(Slot::Failed(_))) {
                state.slots.remove(&hash);
            }
        } else {
            self.shared.invalidate(hash, &status);
        }
        Ok(status)
    }

    /// Drops the hash's peaks from memory, cancelling a build in progress.
    /// The peak file stays, and the next request loads it.
    pub fn forget(&self, hash: ContentHash) {
        let removed = self.shared.state().slots.remove(&hash);
        if let Some(Slot::Pending(progress)) = removed {
            progress.cancel();
        }
        self.shared.settled.notify_all();
    }

    /// Drops the hash's peaks from memory and deletes its peak file.
    pub fn remove(&self, hash: ContentHash) {
        self.forget(hash);
        self.shared.delete_file(hash);
    }

    /// A channel that receives an event whenever peaks become ready, fail,
    /// or are invalidated. Events stop when the receiver is dropped.
    #[must_use]
    pub fn subscribe(&self) -> Receiver<PeakEvent> {
        let (sender, receiver) = mpsc::channel();
        self.shared
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(sender);
        receiver
    }

    /// Deletes the least recently used peak files until those left take at
    /// most `max_bytes`, sparing the files of hashes held in memory. Also
    /// deletes temporary files left over from writes that stopped part-way.
    ///
    /// # Errors
    ///
    /// Fails if the cache folder can't be listed. Files that can't be
    /// deleted are counted as kept.
    pub fn prune(&self, max_bytes: u64) -> io::Result<PruneReport> {
        let Some(dir) = self.dir() else {
            return Ok(PruneReport::default());
        };
        let held: Vec<ContentHash> = self.shared.state().slots.keys().copied().collect();
        let mut files = Vec::new();
        let listing = match fs::read_dir(dir) {
            Ok(listing) => listing,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(PruneReport::default()),
            Err(e) => return Err(e),
        };
        let stale = SystemTime::now()
            .checked_sub(Duration::from_secs(3_600))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        for folder in listing.filter_map(Result::ok) {
            let Ok(children) = fs::read_dir(folder.path()) else {
                continue;
            };
            for child in children.filter_map(Result::ok) {
                let path = child.path();
                let Ok(metadata) = child.metadata() else {
                    continue;
                };
                let used = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let name = child.file_name();
                if is_temporary(&name) {
                    if used < stale {
                        let _ = fs::remove_file(&path);
                    }
                    continue;
                }
                let Some(hash) = hash_of_file_name(&name) else {
                    continue;
                };
                files.push((used, metadata.len(), hash, path));
            }
        }
        files.sort();
        let mut report = PruneReport {
            bytes_kept: files.iter().map(|f| f.1).sum(),
            ..PruneReport::default()
        };
        for (_, size, hash, path) in files {
            if report.bytes_kept <= max_bytes {
                break;
            }
            if held.contains(&hash) || fs::remove_file(&path).is_err() {
                continue;
            }
            report.files_removed += 1;
            report.bytes_removed += size;
            report.bytes_kept -= size;
        }
        Ok(report)
    }

    /// Adds a pending slot for the hash and queues its job.
    fn start(&self, state: &mut State, hash: ContentHash) -> Arc<BuildProgress> {
        let progress = Arc::new(BuildProgress::new());
        state
            .slots
            .insert(hash, Slot::Pending(Arc::clone(&progress)));
        state.running += 1;
        let shared = Arc::clone(&self.shared);
        let job = Arc::clone(&progress);
        self.pool.spawn(move || {
            // A decoder that panics on a damaged file fails this build only.
            let result = panic::catch_unwind(AssertUnwindSafe(|| shared.produce(hash, &job)))
                .unwrap_or_else(|_| {
                    Err(MediaError::Malformed(
                        "decoding panicked while building peaks".into(),
                    ))
                });
            shared.settle(hash, &job, result);
        });
        progress
    }
}

impl Drop for PeakCache {
    fn drop(&mut self) {
        let mut state = self.shared.state();
        for slot in state.slots.values() {
            if let Slot::Pending(progress) = slot {
                progress.cancel();
            }
        }
        while state.running > 0 {
            state = self
                .shared
                .settled
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Slot {
    fn status(&self) -> PeakStatus {
        match self {
            Self::Pending(progress) => PeakStatus::Pending {
                frames_done: progress.frames_done(),
                frames_total: progress.frames_total(),
            },
            Self::Ready(peaks) => PeakStatus::Ready(Arc::clone(peaks)),
            Self::Failed(error) => PeakStatus::Failed(Arc::clone(error)),
        }
    }
}

impl Shared {
    /// Loads the hash's peaks from disk, or builds and saves them.
    fn produce(&self, hash: ContentHash, progress: &BuildProgress) -> Result<Peaks, MediaError> {
        if progress.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        // Peaks are served only for media whose file still holds it, so a
        // changed file's old waveform is never shown.
        let path = || self.store.entry(hash).map(|e| e.path).unwrap_or_default();
        match self.store.check(hash)? {
            MediaStatus::Available => {}
            MediaStatus::Missing => return Err(MediaError::Missing { hash, path: path() }),
            MediaStatus::Changed { now } => {
                self.delete_file(hash);
                return Err(MediaError::Changed {
                    hash,
                    path: path(),
                    now,
                });
            }
        }
        if let Some(peaks) = self.load_file(hash) {
            return Ok(peaks);
        }
        let peaks = Peaks::build(&self.store, hash, &self.build, progress)?;
        if progress.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        if let Some(file) = self.file_path(hash) {
            // A cache that can't be written still serves peaks from memory.
            let _ = peaks.write_file(file, hash);
        }
        Ok(peaks)
    }

    /// Records a job's result, if its slot is still the one it was started
    /// for, and tells waiters and subscribers.
    fn settle(
        &self,
        hash: ContentHash,
        progress: &Arc<BuildProgress>,
        result: Result<Peaks, MediaError>,
    ) {
        let event = {
            let mut state = self.state();
            state.running -= 1;
            let current = matches!(
                state.slots.get(&hash),
                Some(Slot::Pending(p)) if Arc::ptr_eq(p, progress)
            );
            match result {
                _ if !current => None,
                Ok(peaks) => {
                    state.slots.insert(hash, Slot::Ready(Arc::new(peaks)));
                    Some(PeakEvent::Ready(hash))
                }
                Err(MediaError::Cancelled) => {
                    state.slots.remove(&hash);
                    None
                }
                Err(error) => {
                    state.slots.insert(hash, Slot::Failed(Arc::new(error)));
                    Some(PeakEvent::Failed(hash))
                }
            }
        };
        // Tell subscribers before waking waiters, so a caller that returns
        // from `wait` and then subscribes never receives this build's event.
        if let Some(event) = event {
            self.emit(&event);
        }
        self.settled.notify_all();
    }

    /// Drops a hash's peaks because its file is missing or changed. A
    /// changed file's peak file is deleted too; a missing file's is kept, in
    /// case the file is found again.
    fn invalidate(&self, hash: ContentHash, status: &MediaStatus) {
        let removed = self.state().slots.remove(&hash);
        if let Some(Slot::Pending(progress)) = &removed {
            progress.cancel();
        }
        let deleted = matches!(status, MediaStatus::Changed { .. }) && self.delete_file(hash);
        // As in `settle`: subscribers hear of it before waiters wake.
        if removed.is_some() || deleted {
            self.emit(&PeakEvent::Invalidated {
                hash,
                status: status.clone(),
            });
        }
        self.settled.notify_all();
    }

    fn load_file(&self, hash: ContentHash) -> Option<Peaks> {
        let path = self.file_path(hash)?;
        match Peaks::read_file(&path, hash) {
            Ok(peaks) => {
                // Mark it used, for pruning.
                if let Ok(file) = fs::File::options().write(true).open(&path) {
                    let _ = file.set_modified(SystemTime::now());
                }
                Some(peaks)
            }
            // Missing, damaged, or from another version: build it again,
            // and the new file replaces it.
            Err(_) => None,
        }
    }

    /// Deletes the hash's peak file. Returns whether there was one.
    fn delete_file(&self, hash: ContentHash) -> bool {
        self.file_path(hash)
            .is_some_and(|path| fs::remove_file(path).is_ok())
    }

    fn file_path(&self, hash: ContentHash) -> Option<PathBuf> {
        let hex = hash.to_string();
        let dir = self.dir.as_ref()?;
        Some(dir.join(&hex[..2]).join(format!("{hex}.{FILE_EXTENSION}")))
    }

    fn emit(&self, event: &PeakEvent) {
        self.listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|listener| listener.send(event.clone()).is_ok());
    }

    // No lock is held across I/O or decoding, and a panic while one is held
    // can't leave the map half-updated, so a poisoned lock is safe to use.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn is_temporary(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with('.') && name.ends_with(".tmp")
}

fn hash_of_file_name(name: &std::ffi::OsStr) -> Option<ContentHash> {
    let name = name.to_str()?;
    let stem = name.strip_suffix(FILE_EXTENSION)?.strip_suffix('.')?;
    stem.parse().ok()
}

/// The platform's folder for Beatlayer's peak files, or `None` if the
/// environment doesn't say where the user's cache folder is:
///
/// - macOS: `~/Library/Caches/io.loombridge.beatlayer/Peaks`
/// - Windows: `%LOCALAPPDATA%\Beatlayer\Cache\Peaks`
/// - Linux and other Unix systems: `$XDG_CACHE_HOME/beatlayer/peaks`, or
///   `~/.cache/beatlayer/peaks` when `XDG_CACHE_HOME` is unset or relative
#[must_use]
pub fn default_cache_dir() -> Option<PathBuf> {
    cache_dir_from(|name| std::env::var_os(name))
}

/// [`default_cache_dir`], with the environment passed in.
fn cache_dir_from(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| {
        var(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    if cfg!(target_os = "macos") {
        Some(absolute("HOME")?.join("Library/Caches/io.loombridge.beatlayer/Peaks"))
    } else if cfg!(windows) {
        Some(absolute("LOCALAPPDATA")?.join(r"Beatlayer\Cache\Peaks"))
    } else {
        let root = absolute("XDG_CACHE_HOME").or_else(|| Some(absolute("HOME")?.join(".cache")))?;
        Some(root.join("beatlayer/peaks"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_folder_follows_the_platform() {
        // Spec: waveform-peaks R3
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        if cfg!(target_os = "macos") {
            assert_eq!(
                cache_dir_from(env(&[("HOME", "/Users/kit")])),
                Some(PathBuf::from(
                    "/Users/kit/Library/Caches/io.loombridge.beatlayer/Peaks"
                ))
            );
            assert_eq!(cache_dir_from(env(&[])), None);
        } else if cfg!(windows) {
            assert_eq!(
                cache_dir_from(env(&[("LOCALAPPDATA", r"C:\Users\kit\AppData\Local")])),
                Some(PathBuf::from(
                    r"C:\Users\kit\AppData\Local\Beatlayer\Cache\Peaks"
                ))
            );
            assert_eq!(cache_dir_from(env(&[])), None);
        } else {
            assert_eq!(
                cache_dir_from(env(&[("HOME", "/home/kit")])),
                Some(PathBuf::from("/home/kit/.cache/beatlayer/peaks"))
            );
            assert_eq!(
                cache_dir_from(env(&[("HOME", "/home/kit"), ("XDG_CACHE_HOME", "/tmp/c")])),
                Some(PathBuf::from("/tmp/c/beatlayer/peaks"))
            );
            // A relative XDG_CACHE_HOME is ignored, as the spec says.
            assert_eq!(
                cache_dir_from(env(&[("HOME", "/home/kit"), ("XDG_CACHE_HOME", "c")])),
                Some(PathBuf::from("/home/kit/.cache/beatlayer/peaks"))
            );
            assert_eq!(cache_dir_from(env(&[])), None);
        }
    }

    #[test]
    fn peak_file_names_hold_the_hash() {
        let hash = ContentHash::of_bytes(b"x");
        let name = OsString::from(format!("{hash}.blpeaks"));
        assert_eq!(hash_of_file_name(&name), Some(hash));
        assert_eq!(hash_of_file_name(&OsString::from("notes.txt")), None);
        assert!(is_temporary(&OsString::from(".abc.blpeaks.12-0.tmp")));
        assert!(!is_temporary(&name));
    }
}
