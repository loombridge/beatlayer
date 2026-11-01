//! Show Similar Files (spec: sound-similarity R11 to R19): the content pane
//! lists the analysed items nearest a reference, most similar first, with
//! the reference as a token in the search field.
//!
//! The list comes from the descriptors the analysis keeps
//! ([`Analysis::nearest`](super::analysis::Analysis::nearest)); nothing is
//! analysed to make it. A saved label made from it keeps the reference
//! (R15), so the label lists the same neighbours again after a restart.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_shell_spec::browser::{ItemKind, Label, ShowSimilarArgs, SimilarItem, SimilarList};

use super::index::{Scope, Snapshot};
use super::{BrowserError, BrowserView, absolute, analysis, db};

/// How many items besides the reference the list holds (R13).
const NEAREST: usize = 100;

/// How far from the reference an item may be (R13).
const WITHIN: f32 = 0.35;

/// A similarity search under way: the reference, and the browser as it was
/// before, which ending it goes back to (R14).
#[derive(Clone, Debug)]
pub(super) struct Similar {
    pub(super) reference: PathBuf,
    before: (Label, String),
}

impl BrowserView {
    /// The file whose neighbours the content pane lists, shown in the search
    /// field as a token (R12); a saved label made from a list names its own.
    #[must_use]
    pub fn similar_reference(&self) -> Option<&Path> {
        self.reference()
    }

    pub(super) fn reference(&self) -> Option<&Path> {
        match &self.label {
            Label::Saved(id) => self
                .saved
                .iter()
                .find(|saved| saved.id == *id)?
                .reference
                .as_deref()
                .map(Path::new),
            _ => self
                .similar
                .as_ref()
                .map(|similar| similar.reference.as_path()),
        }
    }

    /// What kind of reference `path` is, or why it can't be one (R1, R11).
    fn reference_kind(&self, path: &Path) -> Result<ItemKind, String> {
        let kind = self
            .saved_files
            .get(path.to_string_lossy().as_ref())
            .and_then(|row| db::kind_from_name(&row.kind))
            .filter(|kind| analysis::analysed_kind(*kind).is_some());
        match kind {
            Some(kind) if self.analysis.is_analysed(path) => Ok(kind),
            Some(_) if self.analysis.is_settled(path) => Err(
                "it can't be a reference: it is longer than 60 seconds, couldn't be analysed, \
                 or isn't an instrument or drum preset"
                    .to_owned(),
            ),
            Some(_) => Err("it hasn't been analysed yet".to_owned()),
            None => Err(
                "only audio files of 60 seconds or less and instrument and drum presets can be \
                 a reference"
                    .to_owned(),
            ),
        }
    }

    /// The selected item as a reference, or why there isn't one: the reason
    /// the icon is missing and the menu items are disabled (R11).
    ///
    /// # Errors
    ///
    /// The reason, in words.
    pub fn selected_reference(&self) -> Result<PathBuf, String> {
        let [path] = self.selected.as_slice() else {
            return Err("select one sample or preset".to_owned());
        };
        self.reference_kind(path).map(|_| path.clone())
    }

    /// The reference and its nearest items of its own kind, as indices of
    /// `snapshot` with their distances: the reference first (R12, R13).
    fn neighbours(
        &mut self,
        snapshot: &Snapshot,
        reference: &Path,
    ) -> Result<Vec<(u32, f32)>, BrowserError> {
        let kind = self
            .reference_kind(reference)
            .map_err(BrowserError::Unavailable)?;
        let candidates: Vec<(u32, PathBuf)> = snapshot
            .search("", Scope::All, Some(kind))
            .into_iter()
            .map(|hit| (hit, snapshot.path(hit)))
            .collect();
        let first = candidates
            .iter()
            .find(|(_, path)| path == reference)
            .map(|(hit, _)| *hit)
            .ok_or_else(|| {
                BrowserError::Unavailable("it isn't in the browser's places".to_owned())
            })?;
        let mut near = self
            .analysis
            .nearest(
                self.db.as_ref(),
                reference,
                &candidates,
                WITHIN,
                NEAREST + 1,
            )
            .ok_or_else(|| BrowserError::Unavailable("it hasn't been analysed yet".to_owned()))?;
        near.retain(|(hit, _)| *hit != first);
        near.truncate(NEAREST);
        near.insert(0, (first, 0.0));
        Ok(near)
    }

    /// The content pane's rows for a reference: its neighbours, narrowed by
    /// the search words and the filter tags (R12, R14).
    pub(super) fn similar_hits(
        &mut self,
        snapshot: &Arc<Snapshot>,
        reference: &Path,
    ) -> Result<Vec<u32>, BrowserError> {
        let near = self.neighbours(snapshot, reference)?;
        let (words, tags, _) = self.search_terms();
        let keys = (!tags.is_empty()).then(|| self.items_with_tags(&tags));
        Ok(near
            .into_iter()
            .map(|(hit, _)| hit)
            .filter(|&hit| snapshot.matches(hit, &words))
            .filter(|&hit| {
                keys.as_ref().is_none_or(|keys| {
                    keys.contains(&snapshot.path(hit).to_string_lossy().into_owned())
                })
            })
            .collect())
    }

    /// `browser.similar`: the reference and its neighbours (R16).
    pub(super) fn similar_list(
        &mut self,
        path: Option<String>,
    ) -> Result<SimilarList, BrowserError> {
        let reference = match path {
            Some(path) => absolute(&path)?,
            None => self
                .selected_reference()
                .map_err(BrowserError::Unavailable)?,
        };
        let snapshot = Arc::clone(self.index.snapshot());
        let items: Vec<SimilarItem> = self
            .neighbours(&snapshot, &reference)?
            .into_iter()
            .map(|(hit, distance)| SimilarItem {
                path: snapshot.path(hit).to_string_lossy().into_owned(),
                kind: snapshot.entry(hit).kind,
                distance,
            })
            .collect();
        Ok(SimilarList {
            reference: items[0].clone(),
            items,
        })
    }

    /// `browser.show_similar`: lists a reference's neighbours under All with
    /// the reference as the search field's token, or ends that (R12, R14).
    pub(super) fn show_similar(&mut self, args: ShowSimilarArgs) -> Result<(), BrowserError> {
        if args.end == Some(true) {
            self.end_similar();
            self.rebuild();
            return Ok(());
        }
        let reference = match args.path {
            Some(path) => absolute(&path)?,
            None => self
                .selected_reference()
                .map_err(BrowserError::Unavailable)?,
        };
        self.reference_kind(&reference)
            .map_err(BrowserError::Unavailable)?;
        let before = match self.similar.take() {
            Some(similar) => similar.before,
            None => (self.label.clone(), self.search.clone()),
        };
        self.similar = Some(Similar { reference, before });
        self.label = Label::All;
        self.search.clear();
        self.rebuild();
        Ok(())
    }

    /// Ends a similarity search: the browser goes back to what it showed
    /// before, or to All from a saved label that lists neighbours (R14).
    pub(super) fn end_similar(&mut self) {
        if let Some(Similar { before, .. }) = self.similar.take() {
            (self.label, self.search) = before;
        } else if self.reference().is_some() {
            self.label = Label::All;
            self.search.clear();
        }
    }
}
