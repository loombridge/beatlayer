//! Sound analysis in the background (spec: sound-similarity R1 to R8).
//!
//! The browser keeps a descriptor ([`bl_analysis::similarity`]) for every
//! audio file of 60 seconds or less and every instrument and drum preset in
//! its places, in the database, by the content they hold. When indexing has
//! finished with the places, [`Analysis::enqueue`] lists what has no
//! descriptor yet; [`Analysis::poll`], called each frame, gives the worker
//! thread one file at a time.
//!
//! The worker runs at the lowest priority, so analysis takes no more than
//! one core and the interface, search and previews never wait for it. It is
//! given one file and answers before it is given the next, so pausing is
//! the browser not giving it another: the file under way finishes, and the
//! pause lasts until Resume or until Beatlayer quits.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use bl_analysis::similarity::{self, Descriptor};
use bl_media::{ContentHash, MediaStore};
use bl_project::rack_preset::Saved;
use bl_shell_spec::browser::{AnalysisState, ItemKind};

use super::Waker;
use super::db::{AnalysedRow, AnalysisOp, Db};

/// The tempo a preset is rendered at, so its phrase of one bar of four
/// beats lasts one second: middle C held for one second (R2).
const PRESET_BPM: f64 = 240.0;

/// How much of a preset's render is described, in seconds: the note held
/// for one second, then released for one (R2).
const PRESET_SECONDS: f64 = 2.0;

/// What a file's row says it came to.
const DONE: &str = "done";
const SKIPPED: &str = "skipped";
const IGNORED: &str = "ignored";

/// A file or preset waiting to be analysed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Its path.
    pub path: PathBuf,
    /// Its length in bytes.
    pub size: u64,
    /// When it changed, in seconds since the epoch.
    pub modified: i64,
    /// Whether it's a preset, rendered, and not an audio file.
    pub preset: bool,
}

/// What the worker made of an item.
#[derive(Debug)]
enum Outcome {
    /// The content has this descriptor already; nothing was computed.
    Known(String),
    /// A descriptor, newly computed, for this content.
    Computed(String, Descriptor),
    /// It couldn't be analysed.
    Skipped,
    /// It isn't a sound that is analysed.
    Ignored,
}

/// The content keys that have a descriptor, shared with the worker so it
/// doesn't compute one twice.
type Keys = Arc<Mutex<HashSet<String>>>;

/// The thread, and how to reach it.
struct Worker {
    jobs: Sender<Item>,
    results: Receiver<(Item, Outcome)>,
}

/// Sound analysis: what waits, what the worker is at, and what's been done.
pub struct Analysis {
    /// What the database says of each file it has looked at.
    known: HashMap<String, AnalysedRow>,
    keys: Keys,
    queue: VecDeque<Item>,
    current: Option<Item>,
    paused: bool,
    /// How many files may still be given to the worker before the next
    /// call to [`Analysis::release`], when a test paces analysis by hand.
    permits: Option<usize>,
    worker: Option<Worker>,
    media: Option<Arc<MediaStore>>,
    waker: Waker,
    /// Files finished since the queue was last empty, and the files that
    /// were in it then, for the share done.
    finished: usize,
    batch: usize,
    /// Descriptors computed since Beatlayer started.
    computed: usize,
    /// How many known files have a descriptor, and how many were skipped.
    analysed: usize,
    skipped: usize,
    /// The descriptors by content key, read from the database the first
    /// time a similarity search needs them, then kept up to date.
    // shortcut: every descriptor is held in memory (about 800 bytes each), so
    // 100,000 sounds take about 80 MB; page them from the database if that
    // becomes too much.
    //
    // A mutex, so a search that only reads the analysis (the swap buttons'
    // state, drawn each frame) can load them.
    descriptors: Mutex<Descriptors>,
}

/// The descriptors by content key, and whether the database's have been read.
#[derive(Default)]
struct Descriptors {
    by_key: HashMap<String, Descriptor>,
    loaded: bool,
}

impl std::fmt::Debug for Analysis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analysis")
            .field("waiting", &self.queue.len())
            .field("paused", &self.paused)
            .finish_non_exhaustive()
    }
}

impl Analysis {
    /// No analysis yet; it wakes `waker` when the worker has news.
    #[must_use]
    pub fn new(waker: Waker) -> Self {
        Self {
            known: HashMap::new(),
            keys: Keys::default(),
            queue: VecDeque::new(),
            current: None,
            paused: false,
            permits: None,
            worker: None,
            media: None,
            waker,
            finished: 0,
            batch: 0,
            computed: 0,
            analysed: 0,
            skipped: 0,
            descriptors: Mutex::default(),
        }
    }

    /// Takes what the database holds.
    pub fn load(&mut self, rows: Vec<AnalysedRow>, keys: Vec<(String, u32)>) {
        self.known = rows.into_iter().map(|r| (r.path.clone(), r)).collect();
        self.count();
        if let Ok(mut have) = self.keys.lock() {
            *have = keys
                .into_iter()
                .filter(|(_, version)| *version == similarity::VERSION)
                .map(|(key, _)| key)
                .collect();
        }
    }

    /// The media store whose decoded audio the worker reuses.
    pub fn set_media(&mut self, media: Arc<MediaStore>) {
        self.media = Some(media);
    }

    /// Lists what in `files` has no descriptor, or has changed since it had
    /// one, as what's waiting: what was waiting and has gone from the
    /// places leaves the list.
    pub fn enqueue(&mut self, files: Vec<Item>) {
        let keys = self.keys.lock().map(|k| k.clone()).unwrap_or_default();
        let current = self.current.as_ref().map(|c| c.path.clone());
        // A file that has left the places leaves the count.
        let present: HashSet<String> = files
            .iter()
            .map(|item| item.path.to_string_lossy().into_owned())
            .collect();
        self.known.retain(|path, _| present.contains(path));
        self.count();
        self.queue = files
            .into_iter()
            .filter(|item| current.as_ref() != Some(&item.path))
            .filter(|item| {
                let path = item.path.to_string_lossy();
                !self.known.get(path.as_ref()).is_some_and(|row| {
                    row.size == item.size
                        && row.modified == item.modified
                        && match row.status.as_str() {
                            DONE => row.key.as_ref().is_some_and(|key| keys.contains(key)),
                            SKIPPED | IGNORED => true,
                            _ => false,
                        }
                })
            })
            .collect();
        if self.queue.is_empty() && self.current.is_none() {
            (self.finished, self.batch) = (0, 0);
        } else {
            self.batch = self.finished + self.queue.len() + usize::from(self.current.is_some());
        }
    }

    /// Pauses analysis once the file under way is done, or resumes it.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Whether analysis is paused.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Paces analysis by hand, for tests: no file is given to the worker
    /// until [`Analysis::release`] allows it.
    pub fn pace_by_hand(&mut self) {
        self.permits = Some(0);
    }

    /// Lets the worker be given `files` more files, when paced by hand and
    /// not paused.
    pub fn release(&mut self, files: usize) {
        if let (Some(permits), false) = (&mut self.permits, self.paused) {
            *permits += files;
        }
    }

    /// Gives the worker the next file if it's free and analysis isn't
    /// paused, and takes what it has finished, which goes to `db`. Returns
    /// whether anything the Status Bar shows changed.
    pub fn poll(&mut self, db: Option<&Db>) -> bool {
        let mut changed = false;
        let mut ops = Vec::new();
        while let Some((item, outcome)) =
            self.worker.as_ref().and_then(|w| w.results.try_recv().ok())
        {
            changed = true;
            ops.extend(self.finish(&item, outcome));
        }
        if let Some(db) = db
            && !ops.is_empty()
        {
            db.save_analysis(ops);
        }
        if self.current.is_none() && !self.paused && self.permits != Some(0) {
            if let Some(item) = self.queue.pop_front() {
                if self.send(&item) {
                    self.permits = self.permits.map(|p| p - 1);
                    self.current = Some(item);
                    changed = true;
                }
            } else if self.batch > 0 {
                (self.finished, self.batch) = (0, 0);
                changed = true;
            }
        }
        changed
    }

    /// Waits for the file under way to be done, for tests.
    pub fn wait_for_current(&mut self, db: Option<&Db>) {
        if self.current.is_some()
            && let Some((item, outcome)) = self.worker.as_ref().and_then(|w| w.results.recv().ok())
        {
            let ops = self.finish(&item, outcome);
            if let Some(db) = db {
                db.save_analysis(ops.into_iter().collect());
            }
        }
    }

    /// Notes what became of `item`, which the worker has finished, and the
    /// database's rows for it.
    fn finish(&mut self, item: &Item, outcome: Outcome) -> Option<AnalysisOp> {
        self.current = None;
        self.finished += 1;
        let (status, key, descriptor) = match outcome {
            Outcome::Known(key) => (DONE, Some(key), None),
            Outcome::Computed(key, descriptor) => {
                self.computed += 1;
                if let Ok(mut keys) = self.keys.lock() {
                    keys.insert(key.clone());
                }
                self.descriptors
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .by_key
                    .insert(key.clone(), descriptor.clone());
                (
                    DONE,
                    Some(key),
                    Some((similarity::VERSION, descriptor.to_bytes())),
                )
            }
            Outcome::Skipped => (SKIPPED, None, None),
            Outcome::Ignored => (IGNORED, None, None),
        };
        let row = AnalysedRow {
            path: item.path.to_string_lossy().into_owned(),
            size: item.size,
            modified: item.modified,
            status: status.to_owned(),
            key,
        };
        self.known.insert(row.path.clone(), row.clone());
        self.count();
        Some(AnalysisOp::Row { row, descriptor })
    }

    /// Counts the files done and skipped.
    fn count(&mut self) {
        let status = |wanted: &str| self.known.values().filter(|r| r.status == wanted).count();
        (self.analysed, self.skipped) = (status(DONE), status(SKIPPED));
    }

    /// Gives the worker `item`, starting it first if need be.
    fn send(&mut self, item: &Item) -> bool {
        if self.worker.is_none() {
            let (jobs, job_receiver) = mpsc::channel::<Item>();
            let (answers, results) = mpsc::channel();
            let keys = Arc::clone(&self.keys);
            let media = self.media.clone();
            let waker = self.waker.clone();
            let spawned = std::thread::Builder::new()
                .name("bl-browser-analysis".to_owned())
                .spawn(move || {
                    lower_priority();
                    while let Ok(item) = job_receiver.recv() {
                        let outcome = analyse(&item, &keys, media.as_deref());
                        if answers.send((item, outcome)).is_err() {
                            return;
                        }
                        waker.wake();
                    }
                });
            if spawned.is_err() {
                return false;
            }
            self.worker = Some(Worker { jobs, results });
        }
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.jobs.send(item.clone()).is_ok())
    }

    /// Whether `path` has a descriptor: it can be a similarity reference
    /// (spec: sound-similarity R11).
    #[must_use]
    pub fn is_analysed(&self, path: &Path) -> bool {
        self.key_of(path).is_some()
    }

    /// The content key of an analysed file's descriptor: for an audio file
    /// the hash of its bytes, which is the name the media store gives it.
    #[must_use]
    pub fn key_of(&self, path: &Path) -> Option<&str> {
        let row = self.known.get(path.to_string_lossy().as_ref())?;
        if row.status != DONE {
            return None;
        }
        row.key.as_deref()
    }

    /// Whether `path` was looked at and can't be analysed, as opposed to
    /// not being looked at yet.
    #[must_use]
    pub fn is_settled(&self, path: &Path) -> bool {
        self.known
            .get(path.to_string_lossy().as_ref())
            .is_some_and(|row| row.status != DONE)
    }

    /// The items of `candidates` nearest `reference`, most similar first,
    /// with their distances: the reference first, then those under
    /// `within`, at most `limit` (spec: sound-similarity R12, R13, R19).
    /// Descriptors are scaled over the candidates', so kinds stay apart
    /// when the caller passes one kind. `None` when the reference has no
    /// descriptor.
    pub fn nearest<K: Copy>(
        &self,
        db: Option<&Db>,
        reference: &Path,
        candidates: &[(K, PathBuf)],
        within: f32,
        limit: usize,
    ) -> Option<Vec<(K, f32)>> {
        let mut held = self
            .descriptors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !held.loaded
            && let Some(db) = db
        {
            for (key, version, bytes) in db.descriptors() {
                if version == similarity::VERSION
                    && let Some(descriptor) = Descriptor::from_bytes(&bytes)
                {
                    held.by_key.entry(key).or_insert(descriptor);
                }
            }
            held.loaded = true;
        }
        let of = |path: &Path| self.key_of(path).and_then(|key| held.by_key.get(key));
        let reference = of(reference)?;
        let found: Vec<(K, &Descriptor)> = candidates
            .iter()
            .filter_map(|(id, path)| Some((*id, of(path)?)))
            .collect();
        let normaliser = similarity::Normaliser::fit(found.iter().map(|(_, d)| *d));
        let reference = normaliser.apply(reference);
        let mut near: Vec<(K, f32)> = found
            .iter()
            .map(|(id, d)| (*id, similarity::distance(&reference, &normaliser.apply(d))))
            .filter(|(_, distance)| *distance < within)
            .collect();
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        near.truncate(limit);
        Some(near)
    }

    /// The path of an analysed file whose descriptor is kept under `key`.
    #[must_use]
    pub fn path_of_key(&self, key: &str) -> Option<&Path> {
        self.known
            .values()
            .find(|row| row.status == DONE && row.key.as_deref() == Some(key))
            .map(|row| Path::new(row.path.as_str()))
    }

    /// A number that changes when a descriptor is made or a file is
    /// analysed, so a list worked out from them is worked out again.
    #[must_use]
    pub fn version(&self) -> (usize, usize) {
        (self.analysed, self.computed)
    }

    /// What the Status Bar and `browser.analysis_state` say.
    #[must_use]
    pub fn state(&self) -> AnalysisState {
        let waiting = self.queue.len() + usize::from(self.current.is_some());
        AnalysisState {
            analysed: self.analysed,
            waiting,
            skipped: self.skipped,
            paused: self.paused,
            current: self
                .current
                .as_ref()
                .map(|item| item.path.to_string_lossy().into_owned()),
            share: (waiting > 0 && self.batch > 0)
                .then(|| self.finished as f64 / self.batch as f64),
            computed: self.computed,
        }
    }
}

/// Whether an item of the index is analysed, and if so whether it's a
/// preset: audio files and presets are, nothing else.
#[must_use]
pub fn analysed_kind(kind: ItemKind) -> Option<bool> {
    match kind {
        ItemKind::Audio => Some(false),
        ItemKind::Preset => Some(true),
        _ => None,
    }
}

/// Analyses one item on the worker thread.
fn analyse(item: &Item, keys: &Keys, media: Option<&MediaStore>) -> Outcome {
    let known = |key: &str| keys.lock().is_ok_and(|keys| keys.contains(key));
    if item.preset {
        analyse_preset(&item.path, &known)
    } else {
        analyse_audio(&item.path, &known, media)
    }
}

/// An audio file's descriptor, unless it's too long or can't be decoded
/// (R1, R8).
fn analyse_audio(path: &Path, known: &dyn Fn(&str) -> bool, media: Option<&MediaStore>) -> Outcome {
    let info = match bl_media::probe(path) {
        Ok(info) => info,
        Err(_) => return Outcome::Skipped,
    };
    // shortcut: a file that doesn't say how long it is (some MP3s) is decoded
    // up to 10 minutes before it's found too long; stop at 60 seconds if that
    // ever costs.
    let too_long = info.frames.is_some_and(|frames| {
        frames as f64 > similarity::LONGEST_SECONDS * f64::from(info.sample_rate.max(1))
    });
    if too_long {
        return Outcome::Skipped;
    }
    let hash = match ContentHash::of_file(path) {
        Ok(hash) => hash,
        Err(_) => return Outcome::Skipped,
    };
    let key = hash.to_string();
    if known(&key) {
        return Outcome::Known(key);
    }
    // Audio the media store holds decoded is shared, not decoded again.
    let reused = media
        .filter(|store| store.contains(hash))
        .and_then(|store| store.load(hash).ok());
    let data = match reused {
        Some(data) => data,
        None => match super::preview::decode(path, similarity::RATE) {
            Ok(data) => Arc::new(data),
            Err(_) => return Outcome::Skipped,
        },
    };
    match Descriptor::of_sample(&data) {
        Ok(descriptor) => Outcome::Computed(key, descriptor),
        Err(_) => Outcome::Skipped,
    }
}

/// An instrument's or a drum preset's descriptor, from the render the
/// browser previews it with (R2). Other presets aren't analysed (R1).
fn analyse_preset(path: &Path, known: &dyn Fn(&str) -> bool) -> Outcome {
    use bl_model::DeviceRole;
    let saved = match Saved::load(path) {
        Ok(saved) => saved,
        Err(_) => return Outcome::Skipped,
    };
    let instrument = match &saved {
        Saved::Device(preset) => preset.role == DeviceRole::Instrument,
        Saved::Rack(preset) => preset.rack.role == DeviceRole::Instrument,
        Saved::Chain(_) => false,
    };
    if !instrument {
        return Outcome::Ignored;
    }
    let hash = match ContentHash::of_file(path) {
        Ok(hash) => hash,
        Err(_) => return Outcome::Skipped,
    };
    // shortcut: no device has a version of its own yet, so a preset's key
    // names Beatlayer's build; the descriptors of presets are made again
    // after an update.
    let key = ContentHash::of_bytes(
        format!(
            "preset|{hash}|{}|{}",
            bl_project::preset::PRESET_SCHEMA,
            env!("CARGO_PKG_VERSION")
        )
        .as_bytes(),
    )
    .to_string();
    if known(&key) {
        return Outcome::Known(key);
    }
    let cancel = AtomicBool::new(false);
    let data = match super::preview::render_preset(path, similarity::RATE, PRESET_BPM, &cancel) {
        Ok((data, _)) => data,
        Err(_) => return Outcome::Skipped,
    };
    let frames = (PRESET_SECONDS * f64::from(similarity::RATE)) as usize;
    let planes = data
        .channels()
        .map(|c| c[..c.len().min(frames)].to_vec())
        .collect();
    let Ok(data) = bl_media::SampleData::new(data.sample_rate(), planes) else {
        return Outcome::Skipped;
    };
    match Descriptor::of_sample(&data) {
        Ok(descriptor) => Outcome::Computed(key, descriptor),
        Err(_) => Outcome::Skipped,
    }
}

/// Puts the calling thread at the lowest priority, so analysis gives way to
/// everything else (R5).
fn lower_priority() {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: sets the calling thread's quality of service; it takes no
        // pointers and can't fail in a way that matters.
        unsafe {
            libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, 0);
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // SAFETY: `gettid` and `setpriority` take no pointers; a thread's
        // nice value is set on its own id.
        unsafe {
            let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
            libc::setpriority(libc::PRIO_PROCESS, tid, 19);
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_IDLE,
        };
        // SAFETY: the pseudo handle of the calling thread is always valid.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_IDLE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, size: u64) -> Item {
        Item {
            path: PathBuf::from(path),
            size,
            modified: 5,
            preset: false,
        }
    }

    #[test]
    fn a_file_with_a_current_descriptor_isn_t_listed_again() {
        // Spec: sound-similarity R4, R8.
        let mut analysis = Analysis::new(Waker::default());
        let row = |path: &str, status: &str, key: Option<&str>| AnalysedRow {
            path: path.to_owned(),
            size: 10,
            modified: 5,
            status: status.to_owned(),
            key: key.map(str::to_owned),
        };
        analysis.load(
            vec![
                row("/a.wav", DONE, Some("k")),
                row("/b.wav", SKIPPED, None),
                row("/c.wav", DONE, Some("old")),
            ],
            vec![("k".to_owned(), similarity::VERSION), ("old".to_owned(), 0)],
        );
        analysis.enqueue(vec![
            item("/a.wav", 10),
            item("/b.wav", 10),
            // A descriptor of another version is made again.
            item("/c.wav", 10),
            // A changed file is analysed again.
            item("/d.wav", 10),
            item("/a.wav", 11),
        ]);
        let waiting: Vec<_> = analysis
            .queue
            .iter()
            .map(|i| (i.path.clone(), i.size))
            .collect();
        assert_eq!(
            waiting,
            [
                (PathBuf::from("/c.wav"), 10),
                (PathBuf::from("/d.wav"), 10),
                (PathBuf::from("/a.wav"), 11)
            ]
        );
        let state = analysis.state();
        assert_eq!((state.analysed, state.skipped, state.waiting), (2, 1, 3));
    }

    #[test]
    fn pause_gives_the_worker_nothing_more() {
        // Spec: sound-similarity R6.
        let mut analysis = Analysis::new(Waker::default());
        analysis.enqueue(vec![item("/missing-1.wav", 1), item("/missing-2.wav", 1)]);
        analysis.set_paused(true);
        analysis.poll(None);
        assert_eq!(analysis.state().waiting, 2);
        assert!(analysis.state().current.is_none());
        analysis.set_paused(false);
        analysis.poll(None);
        assert_eq!(analysis.state().current.as_deref(), Some("/missing-1.wav"));
        analysis.set_paused(true);
        analysis.wait_for_current(None);
        analysis.poll(None);
        let state = analysis.state();
        assert_eq!((state.waiting, state.skipped, state.paused), (1, 1, true));
        assert!(state.current.is_none());
    }
}
