//! The analysis cache (WP-4.3, the warping spec R20 to R22): what Beatlayer
//! finds in an audio file, built once in the background, kept by the file's
//! content hash, and saved in the platform's cache folder.
//!
//! It follows the peak cache beside it ([`bl_media::PeakCache`]): the first
//! [`AnalysisCache::request`] for a hash starts the analysis on the cache's
//! own threads and returns at once, and later ones return the analysis when
//! it is ready. An analysis names the content it was made from, so it never
//! goes stale on its own. What changes is whether a file still holds that
//! content: an analysis is made and served only for media the store confirms
//! still holds its content, so a changed file is new media and never shows
//! the old file's analysis.
//!
//! The analysis holds the file's transients, its tempo and its beats (WP-4.9
//! added the last two and raised [`FORMAT_VERSION`], so older files are made
//! again). Beatlayer never reads or writes Live's `.asd` files.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use bl_media::peaks::default_cache_dir;
use bl_media::{ContentHash, MediaError, MediaStatus, MediaStore, SampleData, write_atomically};

use crate::onsets;
use crate::tempo::{self, Tempo};

/// The file name extension of analysis files, without the dot.
pub const FILE_EXTENSION: &str = "blanalysis";

/// The analysis file format version this build reads and writes. It goes up
/// whenever the layout or the way an analysis is made changes, so stale
/// files are made again rather than misread.
pub const FORMAT_VERSION: u32 = 2;

const MAGIC: [u8; 8] = *b"BLANLYS\0";
/// Magic, version, a reserved word, the source hash, the checksum, the count.
const HEADER: usize = 8 + 4 + 4 + 32 + 32 + 8;
const CHECKSUM: std::ops::Range<usize> = 48..80;
/// The tempo's four words.
const TEMPO: usize = 32;

/// How many threads the constructors of [`AnalysisOptions`] give analysis.
pub const ANALYSIS_THREADS: usize = 2;

/// What is found in one audio file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    /// The file's transients as times in seconds from its start, in order
    /// (the warping spec R22).
    pub transients: Vec<f64>,
    /// The file's tempo, if it has a rhythm to measure (the warping spec
    /// R30).
    pub tempo: Option<Tempo>,
    /// The times in seconds of the file's beats, in order, from its first
    /// beat to its end: empty without a tempo (R30).
    pub beats: Vec<f64>,
}

impl Analysis {
    /// Analyses decoded audio: the same file gives the same analysis on
    /// every run.
    ///
    /// # Errors
    ///
    /// If the sample rate is under 1 kHz, which the onset analysis can't
    /// use.
    pub fn of(data: &SampleData) -> Result<Self, MediaError> {
        let rate = data.sample_rate();
        if rate < 1_000 {
            return Err(MediaError::Unsupported(format!(
                "a sample rate of {rate} Hz is too low to analyse"
            )));
        }
        // The channels' average, as `Audio::mono` makes it.
        let scale = 1.0 / data.channel_count() as f64;
        let mono: Vec<f64> = (0..data.frames())
            .map(|i| data.channels().map(|c| f64::from(c[i])).sum::<f64>() * scale)
            .collect();
        let function = onsets::onset_function(&mono, rate);
        let transients = onsets::detect(&mono, &function);
        let tempo = tempo::estimate(&function, &transients);
        let seconds = data.frames() as f64 / f64::from(rate);
        let beats = tempo
            .as_ref()
            .map_or_else(Vec::new, |tempo| tempo::beats(tempo, &transients, seconds));
        Ok(Self {
            transients,
            tempo,
            beats,
        })
    }

    /// The analysis as an analysis file: a header, the transients as
    /// little-endian `f64`s, the tempo (its BPM, confidence, fitted count
    /// and first beat, the BPM 0 for none), the beats' count and the beats.
    /// `source` is the content hash of the media it was made from.
    #[must_use]
    pub fn to_file_bytes(&self, source: ContentHash) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(
            HEADER + 8 * self.transients.len() + TEMPO + 8 + 8 * self.beats.len(),
        );
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(source.as_bytes());
        bytes.extend_from_slice(&[0; 32]);
        bytes.extend_from_slice(&(self.transients.len() as u64).to_le_bytes());
        for time in &self.transients {
            bytes.extend_from_slice(&time.to_le_bytes());
        }
        let tempo = self.tempo.unwrap_or(Tempo {
            bpm: 0.0,
            confidence: 0.0,
            fitted: 0,
            first_beat: None,
        });
        // A first beat of NaN stands for none.
        for word in [
            tempo.bpm,
            tempo.confidence,
            tempo.fitted as f64,
            tempo.first_beat.unwrap_or(f64::NAN),
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&(self.beats.len() as u64).to_le_bytes());
        for time in &self.beats {
            bytes.extend_from_slice(&time.to_le_bytes());
        }
        let sum = ContentHash::of_bytes(&bytes);
        bytes[CHECKSUM].copy_from_slice(sum.as_bytes());
        bytes
    }

    /// Reads an analysis file, checking it's whole, current, and made from
    /// the media `source` names.
    ///
    /// # Errors
    ///
    /// [`MediaError::Malformed`] for a file that isn't a whole current
    /// analysis file of `source`.
    pub fn from_file_bytes(bytes: &[u8], source: ContentHash) -> Result<Self, MediaError> {
        parse(bytes, source).map_err(|why| MediaError::Malformed(format!("analysis file: {why}")))
    }
}

fn parse(bytes: &[u8], source: ContentHash) -> Result<Analysis, String> {
    if bytes.len() < HEADER || bytes[..8] != MAGIC {
        return Err("not an analysis file".into());
    }
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
    if word(8) != FORMAT_VERSION {
        return Err(format!("format version {}", word(8)));
    }
    if bytes[16..48] != *source.as_bytes() {
        return Err("made from other media".into());
    }
    let count = u64::from_le_bytes(bytes[80..88].try_into().expect("eight bytes"));
    let transients_end = usize::try_from(count)
        .ok()
        .and_then(|count| count.checked_mul(8))
        .and_then(|len| len.checked_add(HEADER))
        .filter(|end| {
            end.checked_add(TEMPO + 8)
                .is_some_and(|min| min <= bytes.len())
        })
        .ok_or("the length doesn't match the count")?;
    let beats_at = transients_end + TEMPO;
    let beat_count = u64::from_le_bytes(bytes[beats_at..beats_at + 8].try_into().expect("eight"));
    if u64::try_from(bytes.len() - beats_at - 8).ok() != beat_count.checked_mul(8) {
        return Err("the length doesn't match the counts".into());
    }
    let mut zeroed = bytes.to_vec();
    zeroed[CHECKSUM].fill(0);
    if bytes[CHECKSUM] != *ContentHash::of_bytes(&zeroed).as_bytes() {
        return Err("the checksum doesn't match".into());
    }
    let times = |range: &[u8]| -> Vec<f64> {
        let (words, _) = range.as_chunks::<8>();
        words.iter().map(|t| f64::from_le_bytes(*t)).collect()
    };
    let transients = times(&bytes[HEADER..transients_end]);
    let beats = times(&bytes[beats_at + 8..]);
    // Times that are finite, not negative, and in order: a file that
    // passes its checksum but not this was written by a faulty build.
    let in_order = |times: &[f64]| {
        times.iter().all(|t| t.is_finite() && *t >= 0.0)
            && times.windows(2).all(|pair| pair[0] < pair[1])
    };
    if !in_order(&transients) {
        return Err("the transients aren't in order".into());
    }
    if !in_order(&beats) {
        return Err("the beats aren't in order".into());
    }
    let [bpm, confidence, fitted, first] = times(&bytes[transients_end..beats_at])[..] else {
        unreachable!("the tempo is four words");
    };
    if !(bpm.is_finite() && confidence.is_finite() && fitted.is_finite()) {
        return Err("the tempo isn't a number".into());
    }
    let tempo = (bpm > 0.0).then_some(Tempo {
        bpm,
        confidence,
        fitted: fitted as usize,
        first_beat: first.is_finite().then_some(first),
    });
    Ok(Analysis {
        transients,
        tempo,
        beats,
    })
}

/// How an [`AnalysisCache`] is set up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisOptions {
    /// The folder analysis files are saved in, or `None` to keep analyses in
    /// memory only, making them again in each session.
    pub dir: Option<PathBuf>,
    /// Background threads, shared by every analysis. Zero means one per
    /// core; the constructors below use [`ANALYSIS_THREADS`], so analysis
    /// leaves cores to playback.
    pub threads: usize,
}

impl AnalysisOptions {
    /// Options that save analysis files in `dir`.
    #[must_use]
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
            threads: ANALYSIS_THREADS,
        }
    }

    /// Options that keep analyses in memory only.
    #[must_use]
    pub fn memory_only() -> Self {
        Self {
            dir: None,
            threads: ANALYSIS_THREADS,
        }
    }

    /// Options that save analysis files in the platform's cache folder (see
    /// [`default_analysis_dir`]), or keep them in memory if there isn't one.
    #[must_use]
    pub fn platform_default() -> Self {
        Self {
            dir: default_analysis_dir(),
            threads: ANALYSIS_THREADS,
        }
    }
}

/// The platform's folder for Beatlayer's analysis files, beside the peaks
/// ([`default_cache_dir`]) and named the way it is: `Analysis` where that is
/// `Peaks`, `analysis` where it is `peaks`. `None` when there is no cache
/// folder.
#[must_use]
pub fn default_analysis_dir() -> Option<PathBuf> {
    let peaks = default_cache_dir()?;
    let name = if peaks.file_name() == Some(OsStr::new("peaks")) {
        "analysis"
    } else {
        "Analysis"
    };
    Some(peaks.with_file_name(name))
}

/// Where a hash's analysis stands.
#[derive(Clone, Debug)]
pub enum AnalysisStatus {
    /// The analysis, ready to use.
    Ready(Arc<Analysis>),
    /// Being made (or read from disk).
    Pending,
    /// It failed: the media is unknown, missing, changed, or can't be
    /// decoded. It isn't tried again until [`AnalysisCache::check`] finds
    /// the media available, or the hash is forgotten.
    Failed(Arc<MediaError>),
}

#[derive(Clone)]
enum Slot {
    Pending,
    Ready(Arc<Analysis>),
    Failed(Arc<MediaError>),
}

impl Slot {
    fn status(&self) -> AnalysisStatus {
        match self {
            Self::Pending => AnalysisStatus::Pending,
            Self::Ready(analysis) => AnalysisStatus::Ready(Arc::clone(analysis)),
            Self::Failed(error) => AnalysisStatus::Failed(Arc::clone(error)),
        }
    }
}

/// Analyses made in the background and cached in memory and on disk, keyed
/// by content hash. Shareable between threads.
///
/// Analysis allocates freely, and only on the cache's own threads: nothing
/// it does waits on, or is waited on by, an audio thread (the warping spec
/// R28).
pub struct AnalysisCache {
    shared: Arc<Shared>,
    pool: rayon::ThreadPool,
}

struct Shared {
    store: Arc<MediaStore>,
    dir: Option<PathBuf>,
    slots: Mutex<HashMap<ContentHash, Slot>>,
    /// Signalled whenever a slot settles or is removed.
    settled: Condvar,
}

impl std::fmt::Debug for AnalysisCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalysisCache")
            .field("dir", &self.shared.dir)
            .field("slots", &self.shared.slots().len())
            .finish_non_exhaustive()
    }
}

/// Reads the analyses an [`AnalysisCache`] has ready, without starting any
/// or waiting ([`AnalysisCache::reader`]).
#[derive(Clone)]
pub struct AnalysisReader {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for AnalysisReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalysisReader").finish_non_exhaustive()
    }
}

impl AnalysisReader {
    /// The hash's analysis if it's ready.
    #[must_use]
    pub fn get(&self, hash: ContentHash) -> Option<Arc<Analysis>> {
        match self.shared.slots().get(&hash) {
            Some(Slot::Ready(analysis)) => Some(Arc::clone(analysis)),
            _ => None,
        }
    }
}

impl AnalysisCache {
    /// A cache that analyses media in `store`.
    ///
    /// # Errors
    ///
    /// Fails if the background threads can't be started.
    pub fn new(store: Arc<MediaStore>, options: AnalysisOptions) -> Result<Self, MediaError> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(options.threads)
            .thread_name(|i| format!("bl-analysis-{i}"))
            .build()
            .map_err(|e| MediaError::Io(std::io::Error::other(e)))?;
        Ok(Self {
            shared: Arc::new(Shared {
                store,
                dir: options.dir,
                slots: Mutex::new(HashMap::new()),
                settled: Condvar::new(),
            }),
            pool,
        })
    }

    /// The folder analysis files are saved in, if any.
    #[must_use]
    pub fn dir(&self) -> Option<&Path> {
        self.shared.dir.as_deref()
    }

    /// Where a hash's analysis file is, or would be: `<dir>/<first two hex
    /// digits>/<hash>.blanalysis`.
    #[must_use]
    pub fn file_path(&self, hash: ContentHash) -> Option<PathBuf> {
        self.shared.file_path(hash)
    }

    /// The hash's analysis if it's ready; otherwise starts making it, if
    /// that isn't already under way or failed. Never waits.
    pub fn request(&self, hash: ContentHash) -> AnalysisStatus {
        let mut slots = self.shared.slots();
        if let Some(slot) = slots.get(&hash) {
            return slot.status();
        }
        self.start(&mut slots, hash);
        AnalysisStatus::Pending
    }

    /// The hash's analysis if it's ready, without starting one.
    #[must_use]
    pub fn get(&self, hash: ContentHash) -> Option<Arc<Analysis>> {
        match self.shared.slots().get(&hash) {
            Some(Slot::Ready(analysis)) => Some(Arc::clone(analysis)),
            _ => None,
        }
    }

    /// A handle that reads the analyses ready in this cache, cheap to clone
    /// and to send to another thread, which starts none: what the engine's
    /// compiler asks for a Beats clip's transients through.
    #[must_use]
    pub fn reader(&self) -> AnalysisReader {
        AnalysisReader {
            shared: Arc::clone(&self.shared),
        }
    }

    /// The hash's analysis, making it if needed and waiting until it's
    /// ready. For tools and tests; the app uses [`AnalysisCache::request`].
    /// Don't call it from the cache's own threads, which it would be waiting
    /// on.
    ///
    /// # Errors
    ///
    /// Returns the analysis's error, as [`AnalysisStatus::Failed`] holds it.
    pub fn wait(&self, hash: ContentHash) -> Result<Arc<Analysis>, Arc<MediaError>> {
        let mut slots = self.shared.slots();
        loop {
            match slots.get(&hash) {
                Some(Slot::Ready(analysis)) => return Ok(Arc::clone(analysis)),
                Some(Slot::Failed(error)) => return Err(Arc::clone(error)),
                Some(Slot::Pending) => {
                    slots = self
                        .shared
                        .settled
                        .wait(slots)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                None => self.start(&mut slots, hash),
            }
        }
    }

    /// Checks that the hash's source file still holds its content, through
    /// the media store, and drops the hash's analysis if not: from memory,
    /// and, if the file changed, from disk. A hash whose analysis failed may
    /// be made again once its media is available.
    ///
    /// # Errors
    ///
    /// As for [`MediaStore::check`].
    pub fn check(&self, hash: ContentHash) -> Result<MediaStatus, MediaError> {
        let status = self.shared.store.check(hash)?;
        match status {
            MediaStatus::Available => {
                let mut slots = self.shared.slots();
                if matches!(slots.get(&hash), Some(Slot::Failed(_))) {
                    slots.remove(&hash);
                }
            }
            MediaStatus::Missing => self.forget(hash),
            MediaStatus::Changed { .. } => self.remove(hash),
        }
        Ok(status)
    }

    /// Drops the hash's analysis from memory. The analysis file stays, and
    /// the next request loads it. An analysis being made is dropped when it
    /// ends.
    pub fn forget(&self, hash: ContentHash) {
        self.shared.slots().remove(&hash);
        self.shared.settled.notify_all();
    }

    /// Drops the hash's analysis from memory and deletes its file.
    pub fn remove(&self, hash: ContentHash) {
        self.forget(hash);
        self.shared.delete_file(hash);
    }

    /// Adds a pending slot for the hash and queues its job.
    fn start(&self, slots: &mut HashMap<ContentHash, Slot>, hash: ContentHash) {
        slots.insert(hash, Slot::Pending);
        let shared = Arc::clone(&self.shared);
        self.pool.spawn(move || {
            // A decoder that panics on a damaged file fails this analysis
            // only.
            let result = panic::catch_unwind(AssertUnwindSafe(|| shared.produce(hash)))
                .unwrap_or_else(|_| {
                    Err(MediaError::Malformed(
                        "decoding panicked while analysing".into(),
                    ))
                });
            shared.settle(hash, result);
        });
    }
}

impl Shared {
    /// Loads the hash's analysis from disk, or makes and saves it.
    fn produce(&self, hash: ContentHash) -> Result<Analysis, MediaError> {
        self.require_available(hash)?;
        if let Some(analysis) = self.load_file(hash) {
            return Ok(analysis);
        }
        let data = self.store.load(hash)?;
        let analysis = Analysis::of(&data)?;
        // A file that changed while it was read gives no analysis.
        self.require_available(hash)?;
        if let Some(file) = self.file_path(hash) {
            // A cache that can't be written still serves from memory.
            let _ = write_atomically(&file, &[&analysis.to_file_bytes(hash)]);
        }
        Ok(analysis)
    }

    /// Fails unless the media's file still holds its content; a changed
    /// file's analysis file is deleted.
    fn require_available(&self, hash: ContentHash) -> Result<(), MediaError> {
        let path = || self.store.entry(hash).map(|e| e.path).unwrap_or_default();
        match self.store.check(hash)? {
            MediaStatus::Available => Ok(()),
            MediaStatus::Missing => Err(MediaError::Missing { hash, path: path() }),
            MediaStatus::Changed { now } => {
                self.delete_file(hash);
                Err(MediaError::Changed {
                    hash,
                    path: path(),
                    now,
                })
            }
        }
    }

    /// Records a job's result, unless its slot was forgotten meanwhile, and
    /// wakes waiters.
    fn settle(&self, hash: ContentHash, result: Result<Analysis, MediaError>) {
        let mut slots = self.slots();
        if matches!(slots.get(&hash), Some(Slot::Pending)) {
            slots.insert(
                hash,
                match result {
                    Ok(analysis) => Slot::Ready(Arc::new(analysis)),
                    Err(error) => Slot::Failed(Arc::new(error)),
                },
            );
        }
        drop(slots);
        self.settled.notify_all();
    }

    fn load_file(&self, hash: ContentHash) -> Option<Analysis> {
        let path = self.file_path(hash)?;
        // Missing, damaged, or from another version: make it again, and the
        // new file replaces it.
        Analysis::from_file_bytes(&fs::read(path).ok()?, hash).ok()
    }

    fn delete_file(&self, hash: ContentHash) {
        if let Some(path) = self.file_path(hash) {
            let _ = fs::remove_file(path);
        }
    }

    fn file_path(&self, hash: ContentHash) -> Option<PathBuf> {
        let hex = hash.to_string();
        let dir = self.dir.as_ref()?;
        Some(dir.join(&hex[..2]).join(format!("{hex}.{FILE_EXTENSION}")))
    }

    // No lock is held across I/O or decoding, and a panic while one is held
    // can't leave the map half-updated, so a poisoned lock is safe to use.
    fn slots(&self) -> MutexGuard<'_, HashMap<ContentHash, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_analysis_folder_sits_beside_the_peaks() {
        // Spec: warping R21
        if let (Some(peaks), Some(analysis)) = (default_cache_dir(), default_analysis_dir()) {
            assert_eq!(peaks.parent(), analysis.parent());
            assert_ne!(peaks, analysis);
            assert!(analysis.ends_with("Analysis") || analysis.ends_with("analysis"));
        }
    }

    #[test]
    fn a_file_is_read_back_only_if_whole_current_and_of_the_media() {
        // Spec: warping R21
        let source = ContentHash::of_bytes(b"a");
        let analysis = Analysis {
            transients: vec![0.0, 0.25, 1.5],
            tempo: Some(Tempo {
                bpm: 97.5,
                confidence: 0.75,
                fitted: 6,
                first_beat: Some(0.25),
            }),
            beats: vec![0.25, 0.875, 1.5],
        };
        let bytes = analysis.to_file_bytes(source);
        assert_eq!(Analysis::from_file_bytes(&bytes, source).unwrap(), analysis);
        assert!(Analysis::from_file_bytes(&bytes, ContentHash::of_bytes(b"b")).is_err());
        assert!(Analysis::from_file_bytes(&bytes[..bytes.len() - 1], source).is_err());
        for at in [0, 8, 20, 50, 85, 100, bytes.len() - 1] {
            let mut damaged = bytes.clone();
            damaged[at] ^= 1;
            assert!(
                Analysis::from_file_bytes(&damaged, source).is_err(),
                "a flipped bit at {at}"
            );
        }
    }
}
