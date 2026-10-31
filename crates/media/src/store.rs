//! The media store: where Beatlayer keeps track of the audio files a project
//! uses, by content.
//!
//! Clips name media by [`ContentHash`], never by path, and the document never
//! holds samples. The store maps each hash to the file that holds it, reads
//! the file's header once, checks that the file still holds that content
//! before using it, finds a file again by content when it moves, and hands
//! out decoded audio as shared [`SampleData`].
//!
//! Every method may read files, decode, or allocate, so call them from the
//! main thread or a worker, never from an audio thread. The engine receives
//! `Arc<SampleData>` (or, for disk streaming, a [`Decoder`] on the streaming
//! thread) and reads it without locks.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak};
use std::time::SystemTime;

use crate::decode::{Decoder, MediaInfo};
use crate::edits::{Edit, apply};
use crate::error::MediaError;
use crate::hash::ContentHash;
use crate::resample::convert_sample_rate;
use crate::sample_data::SampleData;

/// What the store knows about one piece of media.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaEntry {
    /// The content hash: the media's identity.
    pub hash: ContentHash,
    /// Where the file was last found.
    pub path: PathBuf,
    /// The file's length in bytes. Relinking compares it before hashing.
    pub size: u64,
    /// What the file holds, once its header has been read.
    pub info: Option<MediaInfo>,
}

/// Whether a piece of media is where the store expects it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaStatus {
    /// The file is there and holds the content.
    Available,
    /// There's no file at the path.
    Missing,
    /// The file at the path now holds other content.
    Changed {
        /// The content it holds now.
        now: ContentHash,
    },
}

/// A file's length and modification time: if neither changed since the file
/// was hashed, the hash still holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    size: u64,
    modified: Option<SystemTime>,
}

impl Fingerprint {
    fn of(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            size: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

struct Record {
    entry: MediaEntry,
    /// The fingerprint when the file was last hashed, if it has been.
    verified: Option<Fingerprint>,
    /// The fingerprint and content hash of the file when it was last found
    /// holding other content, so a changed file isn't hashed again until it
    /// changes again.
    changed: Option<(Fingerprint, ContentHash)>,
}

/// The media a project uses, by content hash. Shareable between threads.
#[derive(Default)]
pub struct MediaStore {
    records: RwLock<HashMap<ContentHash, Record>>,
    /// Decoded audio still in use somewhere, by hash and rate, so a second
    /// load shares it instead of decoding again. Weak, so audio nobody uses
    /// is freed.
    loaded: Mutex<HashMap<(ContentHash, u32), Weak<SampleData>>>,
    /// The same with an edit list applied (the multisampler spec, R57), so
    /// the same source with the same edits is rendered once and shared.
    edited: Mutex<HashMap<EditedKey, Weak<SampleData>>>,
}

/// A source, a rate and the edit list applied to it.
type EditedKey = (ContentHash, u32, Vec<Edit>);

impl std::fmt::Debug for MediaStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaStore")
            .field("entries", &self.read().len())
            .finish_non_exhaustive()
    }
}

impl MediaStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Imports a file: hashes its content, reads its header, and records
    /// where it is. Importing a file whose content the store already has
    /// keeps the path the store has, unless that file is gone or no longer
    /// holds the content.
    ///
    /// The file isn't copied or changed.
    ///
    /// # Errors
    ///
    /// Fails if the file can't be read or isn't audio Beatlayer reads.
    pub fn import(&self, path: impl AsRef<Path>) -> Result<ContentHash, MediaError> {
        let path = absolute(path.as_ref())?;
        let before = Fingerprint::of(&path)?;
        let hash = ContentHash::of_file(&path)?;
        let info = *Decoder::open(&path)?.info();
        // A file that changed while it was hashed is hashed again later.
        let verified = (Fingerprint::of(&path)? == before).then_some(before);

        let keeps_its_path = self
            .read()
            .get(&hash)
            .is_some_and(|record| record.entry.path != path)
            && matches!(self.check(hash), Ok(MediaStatus::Available));
        let mut records = self.write();
        if keeps_its_path && let Some(record) = records.get_mut(&hash) {
            record.entry.info = Some(info);
            return Ok(hash);
        }
        records.insert(
            hash,
            Record {
                entry: MediaEntry {
                    hash,
                    path,
                    size: before.size,
                    info: Some(info),
                },
                verified,
                changed: None,
            },
        );
        Ok(hash)
    }

    /// Records a reference the store hasn't seen, as a project file gives it:
    /// a hash, the path to look for it at, and its length. Nothing is read
    /// until the media is used or checked. A reference to media the store
    /// already has changes nothing.
    pub fn add_reference(&self, hash: ContentHash, path: impl Into<PathBuf>, size: u64) {
        self.write().entry(hash).or_insert_with(|| Record {
            entry: MediaEntry {
                hash,
                path: path.into(),
                size,
                info: None,
            },
            verified: None,
            changed: None,
        });
    }

    /// Whether the store has an entry for this hash.
    #[must_use]
    pub fn contains(&self, hash: ContentHash) -> bool {
        self.read().contains_key(&hash)
    }

    /// The entry for a hash.
    #[must_use]
    pub fn entry(&self, hash: ContentHash) -> Option<MediaEntry> {
        self.read().get(&hash).map(|r| r.entry.clone())
    }

    /// Every entry, in no particular order.
    #[must_use]
    pub fn entries(&self) -> Vec<MediaEntry> {
        self.read().values().map(|r| r.entry.clone()).collect()
    }

    /// Checks that the file still holds the media. Cheap when the file's
    /// length and modification time haven't changed since it was last hashed;
    /// otherwise it hashes the file again.
    ///
    /// # Errors
    ///
    /// Fails if the hash is unknown, or on a read error other than the file
    /// being missing.
    pub fn check(&self, hash: ContentHash) -> Result<MediaStatus, MediaError> {
        let (path, verified, changed) = {
            let records = self.read();
            let record = records.get(&hash).ok_or(MediaError::UnknownMedia(hash))?;
            (record.entry.path.clone(), record.verified, record.changed)
        };
        let fingerprint = match Fingerprint::of(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(MediaStatus::Missing),
            Err(e) => return Err(e.into()),
        };
        if verified == Some(fingerprint) {
            return Ok(MediaStatus::Available);
        }
        if let Some((at, now)) = changed
            && at == fingerprint
        {
            return Ok(MediaStatus::Changed { now });
        }
        let now = ContentHash::of_file(&path)?;
        let mut records = self.write();
        if now != hash {
            if let Some(record) = records.get_mut(&hash)
                && record.entry.path == path
            {
                record.changed = Some((fingerprint, now));
            }
            return Ok(MediaStatus::Changed { now });
        }
        if let Some(record) = records.get_mut(&hash)
            && record.entry.path == path
        {
            record.verified = Some(fingerprint);
            record.changed = None;
            record.entry.size = fingerprint.size;
        }
        Ok(MediaStatus::Available)
    }

    /// The header of the media, read now if it hasn't been.
    ///
    /// # Errors
    ///
    /// As for [`MediaStore::open`].
    pub fn info(&self, hash: ContentHash) -> Result<MediaInfo, MediaError> {
        if let Some(info) = self.read().get(&hash).and_then(|r| r.entry.info) {
            return Ok(info);
        }
        Ok(*self.open(hash)?.info())
    }

    /// Opens a decoder on the media, for streaming or a partial read, after
    /// checking the file still holds it.
    ///
    /// # Errors
    ///
    /// Fails if the hash is unknown, the file is missing or changed, or it
    /// can't be decoded.
    pub fn open(&self, hash: ContentHash) -> Result<Decoder, MediaError> {
        let path = self.available_path(hash)?;
        let decoder = Decoder::open(&path)?;
        if let Some(record) = self.write().get_mut(&hash) {
            record.entry.info = Some(*decoder.info());
        }
        Ok(decoder)
    }

    /// The media decoded into memory at its own sample rate. While any copy
    /// of the returned `Arc` lives, loading it again shares it.
    ///
    /// # Errors
    ///
    /// As for [`MediaStore::open`].
    pub fn load(&self, hash: ContentHash) -> Result<Arc<SampleData>, MediaError> {
        let info = self.info(hash)?;
        self.load_at_rate(hash, info.sample_rate)
    }

    /// The media decoded into memory and converted to a sample rate, such as
    /// the engine's. While any copy of the returned `Arc` lives, loading it
    /// again at that rate shares it.
    ///
    /// # Errors
    ///
    /// As for [`MediaStore::open`], or if the rate is zero.
    pub fn load_at_rate(
        &self,
        hash: ContentHash,
        rate: u32,
    ) -> Result<Arc<SampleData>, MediaError> {
        if let Some(data) = self.cached(hash, rate) {
            return Ok(data);
        }
        let decoder = self.open(hash)?;
        let native_rate = decoder.info().sample_rate;
        let native = match self.cached(hash, native_rate) {
            Some(data) => data,
            None => self.remember(hash, native_rate, decoder.decode_to_end()?),
        };
        if native_rate == rate {
            return Ok(native);
        }
        let converted = convert_sample_rate(&native, rate)?;
        Ok(self.remember(hash, rate, converted))
    }

    /// The media with `edits` applied, at its own sample rate (the
    /// multisampler spec, R57). While any copy of the returned `Arc` lives,
    /// loading the same source with the same edits shares it.
    ///
    /// # Errors
    ///
    /// As for [`MediaStore::open`], or if an edit doesn't fit the sample.
    pub fn load_edited(
        &self,
        hash: ContentHash,
        edits: &[Edit],
    ) -> Result<Arc<SampleData>, MediaError> {
        let info = self.info(hash)?;
        self.load_edited_at_rate(hash, info.sample_rate, edits)
    }

    /// The media with `edits` applied (at its own rate, where their ranges
    /// count) and then converted to a sample rate such as the engine's.
    ///
    /// # Errors
    ///
    /// As for [`MediaStore::load_edited`], or if the rate is zero.
    pub fn load_edited_at_rate(
        &self,
        hash: ContentHash,
        rate: u32,
        edits: &[Edit],
    ) -> Result<Arc<SampleData>, MediaError> {
        if edits.is_empty() {
            return self.load_at_rate(hash, rate);
        }
        let key = (hash, rate, edits.to_vec());
        if let Some(data) = self.edited().get(&key).and_then(Weak::upgrade) {
            return Ok(data);
        }
        let native = self.load(hash)?;
        let rendered = apply(&native, edits)?;
        let rendered = if rendered.sample_rate() == rate {
            rendered
        } else {
            convert_sample_rate(&rendered, rate)?
        };
        let mut edited = self.edited();
        edited.retain(|_, weak| weak.strong_count() > 0);
        if let Some(existing) = edited.get(&key).and_then(Weak::upgrade) {
            return Ok(existing);
        }
        let data = Arc::new(rendered);
        edited.insert(key, Arc::downgrade(&data));
        Ok(data)
    }

    /// Looks for a file holding the media under the given folders, searching
    /// each one's subfolders too, and points the entry at the first match.
    /// Only files of the recorded length are hashed. Returns the new path, or
    /// `None` if nothing matched.
    ///
    /// # Errors
    ///
    /// Fails if the hash is unknown. Folders and files that can't be read
    /// are skipped.
    pub fn relink(
        &self,
        hash: ContentHash,
        folders: &[impl AsRef<Path>],
    ) -> Result<Option<PathBuf>, MediaError> {
        let size = self
            .read()
            .get(&hash)
            .map(|r| r.entry.size)
            .ok_or(MediaError::UnknownMedia(hash))?;
        let found = self.relink_sizes(&HashMap::from([(size, vec![hash])]), folders);
        Ok(found.into_iter().next().map(|(_, path)| path))
    }

    /// Looks for the files holding the media whose file is missing or
    /// changed, searching the given folders and their subfolders, and
    /// returns every candidate found for each of them (file-management R14).
    /// A file with one candidate is relinked to it; a file with several
    /// keeps its row, and the person chooses.
    ///
    /// # Errors
    ///
    /// Fails on a read error while checking an entry's current file.
    pub fn relink_candidates(
        &self,
        folders: &[impl AsRef<Path>],
    ) -> Result<Vec<(ContentHash, Vec<PathBuf>)>, MediaError> {
        let hashes: Vec<ContentHash> = self.read().keys().copied().collect();
        let mut wanted: HashMap<u64, Vec<ContentHash>> = HashMap::new();
        for hash in hashes {
            if self.check(hash)? != MediaStatus::Available
                && let Some(entry) = self.entry(hash)
            {
                wanted.entry(entry.size).or_default().push(hash);
            }
        }
        Ok(self.candidates(&wanted, folders))
    }

    /// Looks for files holding the given contents, each a hash and a length,
    /// under the folders and their subfolders, and returns the first file
    /// found for each. Unlike [`MediaStore::relink`] it needs the store to
    /// know nothing of the hashes, so a file a clip file names can be found
    /// before it is imported (file-management R6).
    #[must_use]
    pub fn find_by_content(
        &self,
        wanted: &[(ContentHash, u64)],
        folders: &[impl AsRef<Path>],
    ) -> Vec<(ContentHash, PathBuf)> {
        let mut sizes: HashMap<u64, Vec<ContentHash>> = HashMap::new();
        for &(hash, size) in wanted {
            sizes.entry(size).or_default().push(hash);
        }
        self.candidates(&sizes, folders)
            .into_iter()
            .filter_map(|(hash, mut paths)| (!paths.is_empty()).then(|| (hash, paths.remove(0))))
            .collect()
    }

    /// Walks the folders, hashing files whose length is wanted, and returns
    /// every file found with each wanted content, in the order the folders
    /// are searched.
    fn candidates(
        &self,
        wanted: &HashMap<u64, Vec<ContentHash>>,
        folders: &[impl AsRef<Path>],
    ) -> Vec<(ContentHash, Vec<PathBuf>)> {
        let mut found: HashMap<ContentHash, Vec<PathBuf>> = HashMap::new();
        let mut stack: Vec<PathBuf> = folders.iter().map(|f| f.as_ref().to_owned()).collect();
        stack.reverse();
        while let Some(folder) = stack.pop() {
            let Ok(listing) = fs::read_dir(&folder) else {
                continue;
            };
            let mut children: Vec<_> = listing.filter_map(Result::ok).collect();
            children.sort_by_key(fs::DirEntry::file_name);
            let mut subfolders = Vec::new();
            for child in children {
                // Symbolic links aren't followed, so a link loop can't trap
                // the search.
                let Ok(kind) = child.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    subfolders.push(child.path());
                    continue;
                }
                if !kind.is_file() {
                    continue;
                }
                let Ok(metadata) = child.metadata() else {
                    continue;
                };
                let Some(candidates) = wanted.get(&metadata.len()) else {
                    continue;
                };
                let path = child.path();
                let Ok(hash) = ContentHash::of_file(&path) else {
                    continue;
                };
                if !candidates.contains(&hash) {
                    continue;
                }
                let paths = found.entry(hash).or_default();
                if paths.contains(&path) {
                    continue;
                }
                paths.push(path);
            }
            stack.extend(subfolders.into_iter().rev());
        }
        let mut out: Vec<(ContentHash, Vec<PathBuf>)> = found.into_iter().collect();
        out.sort_by_key(|(hash, _)| *hash);
        out
    }

    /// Walks the folders, hashing files whose length is wanted, and points
    /// each wanted hash at the first file found with that content.
    fn relink_sizes(
        &self,
        wanted: &HashMap<u64, Vec<ContentHash>>,
        folders: &[impl AsRef<Path>],
    ) -> Vec<(ContentHash, PathBuf)> {
        let mut found = Vec::new();
        for (hash, paths) in self.candidates(wanted, folders) {
            let Some(path) = paths.first() else {
                continue;
            };
            self.relocate(hash, path);
            found.push((hash, path.clone()));
        }
        found
    }

    /// Points an entry at a file known to hold its content.
    pub fn relocate(&self, hash: ContentHash, path: &Path) {
        let fingerprint = Fingerprint::of(path).ok();
        if let Some(record) = self.write().get_mut(&hash) {
            record.entry.path = absolute(path).unwrap_or_else(|_| path.to_owned());
            record.verified = fingerprint;
            record.changed = None;
            if let Some(f) = fingerprint {
                record.entry.size = f.size;
            }
        }
    }

    /// The entry's path, after checking the file there holds the media.
    fn available_path(&self, hash: ContentHash) -> Result<PathBuf, MediaError> {
        let path = self
            .read()
            .get(&hash)
            .map(|r| r.entry.path.clone())
            .ok_or(MediaError::UnknownMedia(hash))?;
        match self.check(hash)? {
            MediaStatus::Available => Ok(path),
            MediaStatus::Missing => Err(MediaError::Missing { hash, path }),
            MediaStatus::Changed { now } => Err(MediaError::Changed { hash, path, now }),
        }
    }

    fn cached(&self, hash: ContentHash, rate: u32) -> Option<Arc<SampleData>> {
        self.loaded().get(&(hash, rate)).and_then(Weak::upgrade)
    }

    /// Caches decoded audio, unless another thread got there first, in which
    /// case that copy is shared instead.
    fn remember(&self, hash: ContentHash, rate: u32, data: SampleData) -> Arc<SampleData> {
        let mut loaded = self.loaded();
        loaded.retain(|_, weak| weak.strong_count() > 0);
        if let Some(existing) = loaded.get(&(hash, rate)).and_then(Weak::upgrade) {
            return existing;
        }
        let data = Arc::new(data);
        loaded.insert((hash, rate), Arc::downgrade(&data));
        data
    }

    // Lock helpers. No lock is held across file reads or decoding, and a
    // panic while holding one can't leave the maps half-updated, so a
    // poisoned lock is still safe to use.

    fn read(&self) -> RwLockReadGuard<'_, HashMap<ContentHash, Record>> {
        self.records
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, HashMap<ContentHash, Record>> {
        self.records
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn loaded(&self) -> MutexGuard<'_, HashMap<(ContentHash, u32), Weak<SampleData>>> {
        self.loaded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn edited(&self) -> MutexGuard<'_, HashMap<EditedKey, Weak<SampleData>>> {
        self.edited
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn absolute(path: &Path) -> io::Result<PathBuf> {
    std::path::absolute(path)
}
