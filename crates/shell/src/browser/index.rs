//! The browser's index: every folder and file in the places, by name, built
//! on a background thread, and searched as the person types (spec: browser
//! R9, R10).
//!
//! The index lives in memory. A [`Snapshot`] holds each item once, with its
//! name in lower case for matching, and the order of the items by name, so a
//! search is one pass that keeps the order. The thread publishes a snapshot
//! as it goes, so search sees what's indexed so far, and starts again when
//! the places change.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bl_shell_spec::browser::ItemKind;

use super::Waker;
use super::listing::{file_kind, is_hidden, natural_cmp};

/// One folder or file in the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    /// The folder it's in, an index into [`Snapshot`]'s folders.
    folder: u32,
    /// Its name.
    pub name: Box<str>,
    /// Its name in lower case, for matching.
    lower: Box<str>,
    /// What it is.
    pub kind: ItemKind,
    /// The place it's in, an index into the roots the index was built from.
    pub place: u16,
}

/// Which places a search covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every place.
    All,
    /// One place, by its index among the roots.
    Place(u16),
}

/// The index at one moment: what's been found so far.
#[derive(Debug, Default)]
pub struct Snapshot {
    roots: Vec<PathBuf>,
    folders: Vec<PathBuf>,
    entries: Vec<IndexEntry>,
    /// Entry indices in name order.
    order: Vec<u32>,
    /// Files found.
    pub files: usize,
    /// Folders found, not counting the places themselves.
    pub folder_count: usize,
}

impl Snapshot {
    /// An index of these items, for tests and benchmarks: `items` are paths
    /// below `root`, the only place.
    #[must_use]
    pub fn from_paths(root: &Path, items: impl IntoIterator<Item = (PathBuf, ItemKind)>) -> Self {
        let mut builder = Builder::new(vec![root.to_path_buf()]);
        for (path, kind) in items {
            let parent = path.parent().unwrap_or(root).to_path_buf();
            let folder = builder.folder(parent);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            builder.push(folder, name, kind, 0);
        }
        builder.snapshot()
    }

    /// The roots it was built from, in order.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// How many items it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// An item.
    #[must_use]
    pub fn entry(&self, index: u32) -> &IndexEntry {
        &self.entries[index as usize]
    }

    /// An item's path.
    #[must_use]
    pub fn path(&self, index: u32) -> PathBuf {
        let entry = self.entry(index);
        self.folders[entry.folder as usize].join(&*entry.name)
    }

    /// The folder an item is in, relative to its place, with the place's
    /// name first, as search results show it.
    #[must_use]
    pub fn folder_label(&self, index: u32) -> String {
        let entry = self.entry(index);
        let folder = &self.folders[entry.folder as usize];
        let root = &self.roots[usize::from(entry.place)];
        let place = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match folder.strip_prefix(root) {
            Ok(rest) if rest.as_os_str().is_empty() => place,
            Ok(rest) => format!("{place}/{}", rest.to_string_lossy().replace('\\', "/")),
            Err(_) => folder.to_string_lossy().into_owned(),
        }
    }

    /// The items whose names hold every word of `query` (ignoring case), in
    /// name order, within `scope`, only audio files if `audio_only`. An
    /// empty query matches everything.
    #[must_use]
    pub fn search(&self, query: &str, scope: Scope, audio_only: bool) -> Vec<u32> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.order
            .iter()
            .copied()
            .filter(|&index| {
                let entry = &self.entries[index as usize];
                (!audio_only || entry.kind == ItemKind::Audio)
                    && match scope {
                        Scope::All => true,
                        Scope::Place(place) => entry.place == place,
                    }
                    && words.iter().all(|word| entry.lower.contains(word.as_str()))
            })
            .collect()
    }

    /// Where `path` is among `hits`, if it's there.
    #[must_use]
    pub fn position_of(&self, hits: &[u32], path: &Path) -> Option<usize> {
        let name = path.file_name()?.to_string_lossy();
        hits.iter().position(|&index| {
            let entry = self.entry(index);
            *entry.name == *name && self.path(index) == path
        })
    }
}

/// Builds a snapshot.
struct Builder {
    roots: Vec<PathBuf>,
    folders: Vec<PathBuf>,
    /// Folders by path, for [`Builder::folder`].
    folder_ids: HashMap<PathBuf, u32>,
    entries: Vec<IndexEntry>,
    files: usize,
    folder_count: usize,
}

impl Builder {
    fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            folders: Vec::new(),
            folder_ids: HashMap::new(),
            entries: Vec::new(),
            files: 0,
            folder_count: 0,
        }
    }

    /// A folder's index, adding it if it's new.
    fn folder(&mut self, path: PathBuf) -> u32 {
        if let Some(found) = self.folder_ids.get(&path) {
            return *found;
        }
        let id = self.add_folder(path.clone());
        self.folder_ids.insert(path, id);
        id
    }

    /// Adds a folder the walk found, which no other entry names.
    fn add_folder(&mut self, path: PathBuf) -> u32 {
        self.folders.push(path);
        u32::try_from(self.folders.len() - 1).unwrap_or(u32::MAX)
    }

    fn push(&mut self, folder: u32, name: String, kind: ItemKind, place: u16) {
        if kind == ItemKind::Folder {
            self.folder_count += 1;
        } else {
            self.files += 1;
        }
        self.entries.push(IndexEntry {
            folder,
            lower: name.to_lowercase().into_boxed_str(),
            name: name.into_boxed_str(),
            kind,
            place,
        });
    }

    fn snapshot(&self) -> Snapshot {
        let mut order: Vec<u32> = (0..self.entries.len() as u32).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&self.entries[a as usize], &self.entries[b as usize]);
            natural_cmp(&a.lower, &b.lower).then_with(|| a.name.cmp(&b.name))
        });
        Snapshot {
            roots: self.roots.clone(),
            folders: self.folders.clone(),
            entries: self.entries.clone(),
            order,
            files: self.files,
            folder_count: self.folder_count,
        }
    }
}

/// What the index thread shares with the browser.
#[derive(Default)]
struct Shared {
    latest: Mutex<Option<Arc<Snapshot>>>,
    generation: AtomicU64,
    indexing: AtomicBool,
}

/// The index of the places: the thread that builds it and the snapshot the
/// browser searches.
pub struct Index {
    requests: Option<Sender<Vec<PathBuf>>>,
    shared: Arc<Shared>,
    waker: Waker,
    roots: Vec<PathBuf>,
    current: Arc<Snapshot>,
    seen: u64,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Index")
            .field("roots", &self.roots)
            .field("entries", &self.current.len())
            .field("indexing", &self.indexing())
            .finish_non_exhaustive()
    }
}

/// How often, at most, the thread publishes while it walks.
const PUBLISH_EVERY: Duration = Duration::from_millis(250);

/// How many new items make the thread publish before that.
const PUBLISH_ITEMS: usize = 25_000;

impl Index {
    /// An empty index that wakes `waker` when it has news. Its thread starts
    /// with the first places.
    #[must_use]
    pub fn new(waker: Waker) -> Self {
        Self {
            requests: None,
            shared: Arc::new(Shared::default()),
            waker,
            roots: Vec::new(),
            current: Arc::new(Snapshot::default()),
            seen: 0,
        }
    }

    /// Indexes these places, in order, if they differ from the last ones.
    pub fn set_roots(&mut self, roots: Vec<PathBuf>) {
        if roots != self.roots {
            self.roots = roots;
            self.refresh();
        }
    }

    /// Indexes the places again.
    pub fn refresh(&mut self) {
        if self.roots.is_empty() && self.requests.is_none() {
            self.current = Arc::new(Snapshot::default());
            return;
        }
        if self.requests.is_none() {
            let (sender, receiver) = mpsc::channel();
            let shared = Arc::clone(&self.shared);
            let waker = self.waker.clone();
            let spawned = std::thread::Builder::new()
                .name("bl-browser-index".to_owned())
                .spawn(move || run(&receiver, &shared, &waker));
            if spawned.is_err() {
                return;
            }
            self.requests = Some(sender);
        }
        self.shared.indexing.store(true, Ordering::Release);
        if let Some(sender) = &self.requests {
            let _ = sender.send(self.roots.clone());
        }
    }

    /// Takes the thread's newest snapshot; returns whether there was one.
    pub fn poll(&mut self) -> bool {
        let generation = self.shared.generation.load(Ordering::Acquire);
        if generation == self.seen {
            return false;
        }
        self.seen = generation;
        let latest = self
            .shared
            .latest
            .lock()
            .map(|latest| latest.clone())
            .unwrap_or_default();
        match latest {
            Some(snapshot) if snapshot.roots == self.roots => {
                self.current = snapshot;
                true
            }
            _ => false,
        }
    }

    /// The snapshot searched now.
    #[must_use]
    pub fn snapshot(&self) -> &Arc<Snapshot> {
        &self.current
    }

    /// Whether the thread is still indexing.
    #[must_use]
    pub fn indexing(&self) -> bool {
        self.shared.indexing.load(Ordering::Acquire)
    }

    /// Waits until the thread has indexed every place, or `timeout` passes,
    /// and takes the result. For tests and benchmarks.
    pub fn wait(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll();
            if !self.indexing() {
                self.poll();
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// The index thread: indexes each request's places, starting again when a
/// newer request arrives, until the browser goes away.
fn run(requests: &Receiver<Vec<PathBuf>>, shared: &Shared, waker: &Waker) {
    let Ok(mut roots) = requests.recv() else {
        return;
    };
    loop {
        // Only the newest request counts.
        while let Ok(newer) = requests.try_recv() {
            roots = newer;
        }
        match walk(&roots, requests, shared, waker) {
            Walk::Done => {
                shared.indexing.store(false, Ordering::Release);
                waker.wake();
                match requests.recv() {
                    Ok(next) => roots = next,
                    Err(_) => return,
                }
            }
            Walk::Restart(next) => roots = next,
            Walk::Stop => return,
        }
    }
}

enum Walk {
    Done,
    Restart(Vec<PathBuf>),
    Stop,
}

fn publish(builder: &Builder, shared: &Shared, waker: &Waker) {
    let snapshot = Arc::new(builder.snapshot());
    if let Ok(mut latest) = shared.latest.lock() {
        *latest = Some(snapshot);
    }
    shared.generation.fetch_add(1, Ordering::AcqRel);
    waker.wake();
}

/// Walks every place, folder by folder, publishing as it goes. Links to
/// folders aren't followed, so a loop can't trap it.
fn walk(
    roots: &[PathBuf],
    requests: &Receiver<Vec<PathBuf>>,
    shared: &Shared,
    waker: &Waker,
) -> Walk {
    let mut builder = Builder::new(roots.to_vec());
    let mut published_at = Instant::now();
    let mut published_items = 0;
    let mut checked = 0usize;
    for (place, root) in roots.iter().enumerate() {
        let place = u16::try_from(place).unwrap_or(u16::MAX);
        let mut stack = vec![builder.add_folder(root.clone())];
        while let Some(folder) = stack.pop() {
            let path = builder.folders[folder as usize].clone();
            let Ok(read) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in read.flatten() {
                checked += 1;
                if checked.is_multiple_of(512) {
                    match requests.try_recv() {
                        Ok(next) => return Walk::Restart(next),
                        Err(TryRecvError::Disconnected) => return Walk::Stop,
                        Err(TryRecvError::Empty) => {}
                    }
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if is_hidden(&name) {
                    continue;
                }
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    let child = builder.add_folder(entry.path());
                    builder.push(folder, name, ItemKind::Folder, place);
                    stack.push(child);
                } else if let Some(kind) = file_kind(&name) {
                    builder.push(folder, name, kind, place);
                }
            }
            if builder.entries.len() - published_items >= PUBLISH_ITEMS
                || published_at.elapsed() >= PUBLISH_EVERY
            {
                publish(&builder, shared, waker);
                published_at = Instant::now();
                published_items = builder.entries.len();
            }
        }
    }
    publish(&builder, shared, waker);
    Walk::Done
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(snapshot: &Snapshot, hits: &[u32]) -> Vec<String> {
        hits.iter()
            .map(|&i| snapshot.entry(i).name.to_string())
            .collect()
    }

    // Spec: browser R9.
    #[test]
    fn a_search_needs_every_word() {
        let root = Path::new("/lib");
        let snapshot = Snapshot::from_paths(
            root,
            [
                ("/lib/Drums/Kick 808.wav", ItemKind::Audio),
                ("/lib/Drums/Snare 808.wav", ItemKind::Audio),
                ("/lib/Drums/KICK acoustic.aif", ItemKind::Audio),
                ("/lib/Drums", ItemKind::Folder),
                ("/lib/Song.blset", ItemKind::Set),
            ]
            .map(|(p, k)| (PathBuf::from(p), k)),
        );
        let hits = snapshot.search("kick", Scope::All, false);
        assert_eq!(
            names(&snapshot, &hits),
            ["Kick 808.wav", "KICK acoustic.aif"]
        );
        let hits = snapshot.search("808 KICK", Scope::All, false);
        assert_eq!(names(&snapshot, &hits), ["Kick 808.wav"]);
        assert_eq!(snapshot.search("", Scope::All, true).len(), 3);
        assert_eq!(snapshot.search("drums", Scope::All, false).len(), 1);
        assert!(snapshot.search("kick", Scope::Place(1), false).is_empty());
        let hit = snapshot.search("snare", Scope::Place(0), false)[0];
        assert_eq!(snapshot.path(hit), Path::new("/lib/Drums/Snare 808.wav"));
        assert_eq!(snapshot.folder_label(hit), "lib/Drums");
        assert_eq!(
            snapshot.position_of(&hits, Path::new("/lib/Drums/Kick 808.wav")),
            Some(0)
        );
    }

    // Spec: browser R10. Release builds answer in about a millisecond (the
    // pull request has the measurement); debug builds are far slower, so
    // this only guards against a search that doesn't keep up at all.
    #[test]
    fn a_search_of_100_000_names_keeps_up() {
        let root = Path::new("/lib");
        let words = ["kick", "snare", "hat", "pad", "bass", "vox", "loop", "fx"];
        let snapshot = Snapshot::from_paths(
            root,
            (0..100_000).map(|i| {
                (
                    PathBuf::from(format!(
                        "/lib/Folder {}/{} {} {i}.wav",
                        i % 500,
                        words[i % 8],
                        words[(i / 8) % 8]
                    )),
                    ItemKind::Audio,
                )
            }),
        );
        assert_eq!(snapshot.len(), 100_000);
        let started = Instant::now();
        for query in [
            "k",
            "ki",
            "kic",
            "kick",
            "kick ",
            "kick b",
            "kick ba",
            "kick bass",
        ] {
            let hits = snapshot.search(query, Scope::All, false);
            assert!(!hits.is_empty(), "{query}");
        }
        let each = started.elapsed() / 8;
        assert!(each < Duration::from_millis(250), "{each:?} a keystroke");
        // "kick bass" and "bass kick" names: i is 32 or 4 more than a
        // multiple of 64.
        let expected = (0..100_000).filter(|i| i % 64 == 32 || i % 64 == 4).count();
        assert_eq!(
            snapshot.search("kick bass", Scope::All, false).len(),
            expected
        );
    }
}
