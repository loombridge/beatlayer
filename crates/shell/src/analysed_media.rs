//! The media library the engine plays clips from, with the files'
//! transients the shell's analysis has found (the warp modes spec, R14).
//!
//! A Beats clip cuts its segments at the warp markers and at its file's
//! transients, which the engine asks its [`MediaLibrary`] for. The media
//! store and the disk streams don't analyse; the shell's analysis cache
//! does, and the engine can't depend on `bl-analysis`. Without this the
//! app's Beats clips cut at their warp markers alone, one segment a bar
//! after auto-warp, and filled each bar's stretch from its middle: a bar
//! that played its beats and then some of them again (24 October 2026).

use std::sync::{Arc, RwLock};

use bl_analysis::cache::AnalysisReader;
use bl_core::time::SampleRate;
use bl_engine::{MediaLibrary, OpenedSource};
use bl_media::{ProgressiveSample, SampleData};
use bl_model::{ContentHash, SampleEdit};

/// The shell's analysis cache as the engine's libraries read it: replaced
/// when the shell makes a new cache, empty before the first.
#[derive(Clone, Debug, Default)]
pub struct AnalysisSlot(Arc<RwLock<Option<AnalysisReader>>>);

impl AnalysisSlot {
    /// Reads `reader`'s analyses from now on, or none.
    pub fn set(&self, reader: Option<AnalysisReader>) {
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = reader;
    }

    /// The file's transients in seconds from its start, if its analysis is
    /// ready.
    #[must_use]
    pub fn transients(&self, media: ContentHash) -> Option<Arc<Vec<f64>>> {
        let reader = self
            .0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let analysis = reader
            .as_ref()?
            .get(bl_media::ContentHash::from_bytes(media.0))?;
        Some(Arc::new(analysis.transients.clone()))
    }
}

/// A media library that answers everything from `inner` and the files'
/// transients from the shell's analyses.
pub struct AnalysedMedia {
    inner: Arc<dyn MediaLibrary>,
    analysis: AnalysisSlot,
}

impl AnalysedMedia {
    /// `inner`, with the transients `analysis` reads.
    #[must_use]
    pub fn library(inner: Arc<dyn MediaLibrary>, analysis: AnalysisSlot) -> Arc<dyn MediaLibrary> {
        Arc::new(Self { inner, analysis })
    }
}

impl MediaLibrary for AnalysedMedia {
    fn open(&self, media: ContentHash, rate: SampleRate, start: u64) -> Option<OpenedSource> {
        self.inner.open(media, rate, start)
    }

    fn open_in_memory(&self, media: ContentHash, rate: SampleRate) -> Option<OpenedSource> {
        self.inner.open_in_memory(media, rate)
    }

    fn transients(&self, media: ContentHash) -> Option<Arc<Vec<f64>>> {
        self.analysis
            .transients(media)
            .or_else(|| self.inner.transients(media))
    }

    fn sample(&self, media: ContentHash, rate: SampleRate) -> Option<Arc<SampleData>> {
        self.inner.sample(media, rate)
    }

    fn native_rate(&self, media: ContentHash) -> Option<u32> {
        self.inner.native_rate(media)
    }

    fn edited_sample(
        &self,
        media: ContentHash,
        rate: SampleRate,
        edits: &[SampleEdit],
    ) -> Option<Arc<SampleData>> {
        self.inner.edited_sample(media, rate, edits)
    }

    fn progressive(
        &self,
        media: ContentHash,
        rate: SampleRate,
        head: usize,
    ) -> Option<Arc<ProgressiveSample>> {
        self.inner.progressive(media, rate, head)
    }
}
