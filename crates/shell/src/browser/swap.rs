//! The list similar sample swapping steps through (spec: sound-similarity
//! R21, R27): a sampler's similarity reference first, then the samples most
//! like it, from the same descriptors Show Similar Files lists.
//!
//! A sampler names its reference by content hash, which is the key the
//! analysis keeps an audio file's descriptor under, so the browser finds the
//! reference's file among its places and lists its neighbours as
//! [`BrowserView::similar_list`] does. The list is worked out once for a
//! reference, index and analysis, since the swap buttons ask for it each
//! frame.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use bl_model::ContentHash;

use super::{BrowserError, BrowserView};

/// One sample of a swap list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapItem {
    /// The hash of its bytes: what a sampler names it by.
    pub hash: ContentHash,
    /// The file in the browser's places.
    pub path: PathBuf,
}

/// A reference and its similar samples, the reference first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapList {
    /// The samples, the reference at 0, then the most similar first.
    pub items: Vec<SwapItem>,
}

impl SwapList {
    /// Where the sample named `hash` is in the list.
    #[must_use]
    pub fn position(&self, hash: ContentHash) -> Option<usize> {
        self.items.iter().position(|item| item.hash == hash)
    }
}

/// What the lists were worked out from: the index searched and what the
/// analysis had done.
pub(super) type SwapGeneration = (usize, (usize, usize));

/// The lists worked out so far, by reference, for one generation: a Drum
/// Rack's pads each ask for theirs every frame while Option is held.
#[derive(Debug, Default)]
pub(super) struct SwapCache {
    generation: SwapGeneration,
    lists: HashMap<ContentHash, Result<Arc<SwapList>, String>>,
}

/// How many references' lists are kept at once.
const KEPT: usize = 256;

impl BrowserView {
    /// The samples most like `reference`, the reference first (R21).
    ///
    /// # Errors
    ///
    /// Why there is no list: the reference isn't among the analysed
    /// samples of the browser's places, so R27's controls are disabled.
    pub fn swap_list(&self, reference: ContentHash) -> Result<Arc<SwapList>, String> {
        let snapshot = Arc::clone(self.index.snapshot());
        let generation = (Arc::as_ptr(&snapshot) as usize, self.analysis.version());
        let mut cache = self
            .swaps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.generation != generation || cache.lists.len() >= KEPT {
            cache.generation = generation;
            cache.lists.clear();
        }
        if let Some(made) = cache.lists.get(&reference) {
            return made.clone();
        }
        let made = self.make_swap_list(&snapshot, reference);
        cache.lists.insert(reference, made.clone());
        made
    }

    fn make_swap_list(
        &self,
        snapshot: &Arc<super::index::Snapshot>,
        reference: ContentHash,
    ) -> Result<Arc<SwapList>, String> {
        let path = self
            .analysis
            .path_of_key(&reference.to_string())
            .map(std::path::Path::to_path_buf)
            .ok_or_else(|| {
                "its reference isn't an analysed sample in the browser's places".to_owned()
            })?;
        let near = self
            .neighbours(snapshot, &path)
            .map_err(|error| match error {
                BrowserError::InvalidArgs(why)
                | BrowserError::Unavailable(why)
                | BrowserError::Failed(why) => why,
            })?;
        let items = near
            .into_iter()
            .filter_map(|(hit, _)| {
                let path = snapshot.path(hit);
                let hash = ContentHash::from_str(self.analysis.key_of(&path)?).ok()?;
                Some(SwapItem { hash, path })
            })
            .collect();
        Ok(Arc::new(SwapList { items }))
    }
}
