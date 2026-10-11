//! Waveform peaks for the Arrangement View: where they come from, which are
//! on the GPU, and what to redraw when they change (arrangement-view R4).
//!
//! A [`PeakSource`] hands out peaks by content hash without waiting: the
//! app's is `bl-media`'s [`PeakCache`], which builds them in the background.
//! The [`PeakLibrary`] remembers each media file's state, asks the source
//! once, and listens to the cache's events: when peaks become ready it
//! places them in the [`Atlas`], the block the waveform shader reads, and
//! when a file's peaks are invalidated it drops them, so the clip draws
//! without a waveform until the new ones are built.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use bl_media::{PeakCache, PeakEvent, PeakStatus, Peaks};
use bl_model::ContentHash;

/// Where a media file's peaks stand.
#[derive(Clone, Debug)]
pub enum PeakState {
    /// Ready to draw.
    Ready(Arc<Peaks>),
    /// Being built or loaded.
    Pending,
    /// They can't be built: the media is missing, changed, or undecodable.
    Failed,
}

/// Hands out peaks by content hash, never waiting.
pub trait PeakSource {
    /// The peaks if they're ready; otherwise starts building them if
    /// nothing has, and says so.
    fn request(&self, media: ContentHash) -> PeakState;
}

/// `bl-media`'s ContentHash for the document's.
fn media_hash(hash: ContentHash) -> bl_media::ContentHash {
    bl_media::ContentHash::from_bytes(hash.0)
}

/// The document's ContentHash for `bl-media`'s.
fn model_hash(hash: bl_media::ContentHash) -> ContentHash {
    ContentHash(*hash.as_bytes())
}

impl PeakSource for PeakCache {
    fn request(&self, media: ContentHash) -> PeakState {
        match PeakCache::request(self, media_hash(media)) {
            PeakStatus::Ready(peaks) => PeakState::Ready(peaks),
            PeakStatus::Pending { .. } => PeakState::Pending,
            PeakStatus::Failed(_) => PeakState::Failed,
        }
    }
}

impl<T: PeakSource + ?Sized> PeakSource for Arc<T> {
    fn request(&self, media: ContentHash) -> PeakState {
        (**self).request(media)
    }
}

/// Peaks held in memory, for tests and benchmarks: every hash it has is
/// ready, every other one has failed.
#[derive(Clone, Debug, Default)]
pub struct MemoryPeaks {
    peaks: HashMap<ContentHash, Arc<Peaks>>,
}

impl MemoryPeaks {
    /// An empty set of peaks.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a media file's peaks.
    pub fn insert(&mut self, media: ContentHash, peaks: Arc<Peaks>) {
        self.peaks.insert(media, peaks);
    }

    /// How many files it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peaks.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peaks.is_empty()
    }
}

impl PeakSource for MemoryPeaks {
    fn request(&self, media: ContentHash) -> PeakState {
        self.peaks.get(&media).map_or(PeakState::Failed, |peaks| {
            PeakState::Ready(Arc::clone(peaks))
        })
    }
}

/// A hasher for content hashes, which are already uniformly random: their
/// first eight bytes.
#[derive(Default)]
pub struct ContentHasher(u64);

impl Hasher for ContentHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.0 = self.0.rotate_left(5) ^ u64::from_le_bytes(word);
        }
    }
}

/// A map keyed by content hash.
pub type HashMapByContent<V> = HashMap<ContentHash, V, BuildHasherDefault<ContentHasher>>;

/// One file's peaks in the atlas.
#[derive(Clone, Debug)]
pub struct AtlasEntry {
    /// The media.
    pub media: ContentHash,
    /// Where its data block starts, in 32-bit words.
    pub word_offset: u64,
    /// The peaks.
    pub peaks: Arc<Peaks>,
    /// When it was added, counting up, so the GPU side uploads only what's
    /// new.
    pub seq: u64,
}

impl AtlasEntry {
    /// Where its data block starts, in bytes.
    #[must_use]
    pub fn byte_offset(&self) -> u64 {
        self.word_offset * 4
    }

    /// The index of its block's first peak in the buffer: two peaks to a
    /// word.
    #[must_use]
    pub fn peak_base(&self) -> u64 {
        self.word_offset * 2
    }
}

/// The contents of the peak buffer the waveform shader reads: data blocks
/// one after another, each where a word starts. A dropped block leaves a
/// gap until the atlas fills up, when it packs the rest together again
/// under a new epoch, which tells the GPU side to upload everything.
#[derive(Clone, Debug)]
pub struct Atlas {
    epoch: u64,
    entries: Vec<AtlasEntry>,
    next_word: u64,
    next_seq: u64,
    limit_bytes: u64,
    gap_words: u64,
}

impl Default for Atlas {
    fn default() -> Self {
        Self {
            epoch: 0,
            entries: Vec::new(),
            next_word: 0,
            next_seq: 1,
            limit_bytes: super::gpu::MAX_PEAK_BYTES,
            gap_words: 0,
        }
    }
}

impl Atlas {
    /// Changes when the blocks move, so everything must be uploaded again.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The bytes the blocks span, gaps included.
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.next_word * 4
    }

    /// The newest entry's sequence number, or 0.
    #[must_use]
    pub fn last_seq(&self) -> u64 {
        self.next_seq - 1
    }

    /// The most bytes it may span, from the GPU's limits.
    pub fn set_limit(&mut self, bytes: u64) {
        self.limit_bytes = bytes;
    }

    /// The entries added after `seq`, or all of them.
    pub fn entries_after(&self, seq: Option<u64>) -> impl Iterator<Item = &AtlasEntry> {
        self.entries
            .iter()
            .filter(move |entry| seq.is_none_or(|after| entry.seq > after))
    }

    /// How many files it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds a file's peaks and returns where its first peak is, or `None` if
    /// they don't fit even after packing.
    pub fn insert(&mut self, media: ContentHash, peaks: Arc<Peaks>) -> Option<AtlasEntry> {
        let words = (peaks.data().len() as u64).div_ceil(4);
        let limit_words = self.limit_bytes / 4;
        if self.next_word + words > limit_words && self.gap_words > 0 {
            self.pack();
        }
        if self.next_word + words > limit_words {
            return None;
        }
        let entry = AtlasEntry {
            media,
            word_offset: self.next_word,
            peaks,
            seq: self.next_seq,
        };
        self.next_word += words;
        self.next_seq += 1;
        self.entries.push(entry.clone());
        Some(entry)
    }

    /// Drops a file's peaks.
    pub fn remove(&mut self, media: ContentHash) {
        if let Some(at) = self.entries.iter().position(|e| e.media == media) {
            let entry = self.entries.remove(at);
            self.gap_words += (entry.peaks.data().len() as u64).div_ceil(4);
        }
    }

    /// Packs the blocks together, moving them, under a new epoch.
    fn pack(&mut self) {
        self.epoch += 1;
        self.next_word = 0;
        self.gap_words = 0;
        for entry in &mut self.entries {
            entry.word_offset = self.next_word;
            entry.seq = self.next_seq;
            self.next_seq += 1;
            self.next_word += (entry.peaks.data().len() as u64).div_ceil(4);
        }
    }

    /// Where each file's first peak is, after a pack.
    pub fn bases(&self) -> impl Iterator<Item = (ContentHash, u64)> + '_ {
        self.entries.iter().map(|e| (e.media, e.peak_base()))
    }
}

/// A file's peaks as the library knows them.
#[derive(Clone, Debug)]
pub enum Known {
    /// In the atlas: the peaks and where their first peak is.
    Ready {
        /// The peaks.
        peaks: Arc<Peaks>,
        /// The index of the block's first peak in the peak buffer.
        base: u64,
    },
    /// Asked for and not ready.
    Pending,
    /// Failed, or too big for the peak buffer.
    Failed,
}

/// What the Arrangement View knows about every file's peaks. See the
/// [module docs](self).
pub struct PeakLibrary {
    source: Option<Box<dyn PeakSource>>,
    events: Option<Receiver<PeakEvent>>,
    known: HashMapByContent<Known>,
    /// Changes whenever a file's state does, so the view rebuilds.
    generation: u64,
    frames_since_poll: u32,
}

impl std::fmt::Debug for PeakLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeakLibrary")
            .field("known", &self.known.len())
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl Default for PeakLibrary {
    fn default() -> Self {
        Self::new()
    }
}

/// How often, in frames, pending files are asked about again when no event
/// says they're ready: a fallback for sources without events.
const POLL_FRAMES: u32 = 30;

impl PeakLibrary {
    /// A library with no source: every clip draws without a waveform.
    #[must_use]
    pub fn new() -> Self {
        Self {
            source: None,
            events: None,
            known: HashMapByContent::default(),
            generation: 0,
            frames_since_poll: 0,
        }
    }

    /// Takes peaks from `source`, forgetting what came from the last one.
    /// `events` (from [`PeakCache::subscribe`]) says when peaks become ready
    /// or are invalidated.
    pub fn set_source(
        &mut self,
        source: Box<dyn PeakSource>,
        events: Option<Receiver<PeakEvent>>,
        atlas: &mut Atlas,
    ) {
        self.source = Some(source);
        self.events = events;
        for media in self.known.keys() {
            atlas.remove(*media);
        }
        self.known.clear();
        self.generation += 1;
    }

    /// Changes whenever any file's peaks change.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Takes the source's events, once per frame: ready peaks are asked
    /// for again, invalidated ones dropped. Returns whether anything
    /// changed.
    pub fn update(&mut self, atlas: &mut Atlas) -> bool {
        let before = self.generation;
        let mut recheck = Vec::new();
        if let Some(events) = &self.events {
            while let Ok(event) = events.try_recv() {
                match event {
                    PeakEvent::Ready(hash) => recheck.push(model_hash(hash)),
                    PeakEvent::Failed(hash) => {
                        self.known.insert(model_hash(hash), Known::Failed);
                        self.generation += 1;
                    }
                    PeakEvent::Invalidated { hash, .. } => {
                        // Spec: arrangement-view R4.
                        let media = model_hash(hash);
                        atlas.remove(media);
                        self.known.remove(&media);
                        self.generation += 1;
                    }
                }
            }
        }
        self.frames_since_poll += 1;
        if self.frames_since_poll >= POLL_FRAMES {
            self.frames_since_poll = 0;
            recheck.extend(
                self.known
                    .iter()
                    .filter(|(_, known)| matches!(known, Known::Pending))
                    .map(|(media, _)| *media),
            );
        }
        for media in recheck {
            if let Some(Known::Pending) = self.known.get(&media) {
                self.known.remove(&media);
                self.ask(media, atlas);
            }
        }
        self.generation != before
    }

    /// What's known about a file's peaks, asking the source the first time.
    pub fn get(&mut self, media: ContentHash, atlas: &mut Atlas) -> &Known {
        if !self.known.contains_key(&media) {
            self.ask(media, atlas);
        }
        self.known.get(&media).unwrap_or(&Known::Failed)
    }

    /// What's known about a file's peaks, without asking.
    #[must_use]
    pub fn peek(&self, media: ContentHash) -> Option<&Known> {
        self.known.get(&media)
    }

    fn ask(&mut self, media: ContentHash, atlas: &mut Atlas) {
        let Some(source) = &self.source else {
            self.known.insert(media, Known::Failed);
            return;
        };
        let known = match source.request(media) {
            PeakState::Ready(peaks) => {
                let epoch = atlas.epoch();
                match atlas.insert(media, Arc::clone(&peaks)) {
                    Some(entry) => {
                        if atlas.epoch() != epoch {
                            // The atlas packed: every block moved.
                            self.rebase(atlas);
                        }
                        Known::Ready {
                            peaks,
                            base: entry.peak_base(),
                        }
                    }
                    None => Known::Failed,
                }
            }
            PeakState::Pending => Known::Pending,
            PeakState::Failed => Known::Failed,
        };
        self.known.insert(media, known);
        self.generation += 1;
    }

    fn rebase(&mut self, atlas: &Atlas) {
        for (media, base) in atlas.bases() {
            if let Some(Known::Ready { base: old, .. }) = self.known.get_mut(&media) {
                *old = base;
            }
        }
    }
}
