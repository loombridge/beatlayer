//! Beatlayer's library in the browser (spec: core-library R8 to R11; browser
//! R31): the Core Library label, the library's items under the content
//! labels beside the person's own, and their tags.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bl_shell_spec::browser::{Item, ItemKind, Label};

use super::filter::{BuiltinIds, Selection};
use super::tags::AutoSource;
use super::{BrowserView, listing};

/// The tag groups the library fills (R9): Beatlayer's own, shown beside the
/// default ones under every label.
pub const GROUPS: [&str; 4] = ["Genre", "Instrument", "Character", "Source"];

/// An installed item of the library, as the browser lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryItem {
    /// Its file.
    pub path: PathBuf,
    /// Its name.
    pub name: String,
    /// The content label it lists under, by name.
    pub label: String,
    /// Its tags, each `Group|Tag` or `Group|Tag|Subtag`.
    pub tags: Vec<String>,
    /// The name of its package, shown beside it in a search.
    pub package: String,
}

/// What the browser knows of the library.
#[derive(Clone, Debug, Default)]
pub struct LibraryView {
    /// The folder the packages are installed in: indexed, so search finds
    /// the items, but not listed as a place (R7).
    dir: Option<PathBuf>,
    items: Vec<LibraryItem>,
}

impl LibraryView {
    /// The folder the library is indexed from.
    #[must_use]
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }
}

/// The content label of that name.
fn content_label(name: &str) -> Option<Label> {
    Label::LIBRARY
        .into_iter()
        .find(|label| label.name() == name)
}

impl BrowserView {
    /// Tells the browser which library items are installed, in which folder
    /// (R8, R9): their tags are given to them as automatic tags, which a
    /// person can take off, and the folder is indexed with the places.
    pub fn set_library(&mut self, dir: Option<PathBuf>, items: Vec<LibraryItem>) {
        let moved = self.library.dir != dir;
        let kept: std::collections::HashSet<&Path> =
            items.iter().map(|item| item.path.as_path()).collect();
        let gone: Vec<String> = self
            .library
            .items
            .iter()
            .filter(|old| !kept.contains(old.path.as_path()))
            .map(|old| old.path.to_string_lossy().into_owned())
            .collect();
        self.library.dir = dir;
        self.library.items = items;
        let mut ops = Vec::new();
        for key in gone {
            ops.extend(self.tags.set_auto_tags(&key, AutoSource::Library, []));
        }
        for item in &self.library.items {
            let ids: BTreeSet<i64> = item
                .tags
                .iter()
                .filter_map(|tag| self.tags.ensure_path(tag, &mut ops))
                .collect();
            ops.extend(self.tags.set_auto_tags(
                &item.path.to_string_lossy(),
                AutoSource::Library,
                ids,
            ));
        }
        self.save_tags(ops);
        if moved {
            self.places_changed();
        } else {
            self.generation += 1;
            self.rebuild();
        }
    }

    /// Whether any library item is installed.
    #[must_use]
    pub fn library_installed(&self) -> bool {
        !self.library.items.is_empty()
    }

    /// The installed library item whose file is `path`.
    #[must_use]
    pub fn library_item(&self, path: &Path) -> Option<&LibraryItem> {
        self.library.items.iter().find(|item| item.path == path)
    }

    /// The library items an item of `label` lists: all of them under Core
    /// Library, the ones with the label's name under a content label that
    /// isn't searched through the index.
    pub(super) fn library_items_of(&self, label: &Label) -> Vec<&LibraryItem> {
        self.library
            .items
            .iter()
            .filter(|item| match label {
                Label::CoreLibrary => true,
                Label::Samples | Label::Clips => false,
                other => content_label(&item.label).as_ref() == Some(other),
            })
            .collect()
    }

    /// An item's row; in a search it names its package.
    pub(super) fn library_row(&self, item: &LibraryItem, searching: bool) -> Item {
        Item {
            path: item.path.to_string_lossy().into_owned(),
            name: item.name.clone(),
            kind: listing::path_kind(&item.path).unwrap_or(ItemKind::Audio),
            depth: 0,
            open: false,
            folder: searching.then(|| item.package.clone()),
            format: None,
            clip_kind: listing::clip_kind(&item.path),
            details: None,
            collections: Vec::new(),
            hidden: false,
            presets: false,
            nested: Vec::new(),
        }
    }

    /// The rows of the Core Library label and of the content labels the
    /// library has items for (R8, R10), or `None` if `label` lists none of
    /// the library's: every word searched must be in the name, every `#`
    /// tag carried, and the Filter View's selection matched.
    pub(super) fn library_rows(
        &self,
        label: &Label,
        selection: &Selection,
        builtin: &BuiltinIds,
    ) -> Option<Vec<Item>> {
        // The Grooves label lists them with its own and the index finds the
        // rest under Samples and Clips.
        if *label == Label::Grooves {
            return None;
        }
        let items = self.library_items_of(label);
        if items.is_empty() && *label != Label::CoreLibrary {
            return None;
        }
        let (words, tags, _) = self.search_terms();
        let searching = !self.search.trim().is_empty();
        let mut rows: Vec<Item> = items
            .into_iter()
            .filter(|item| {
                let name = item.name.to_lowercase();
                words.iter().all(|word| name.contains(word))
                    && self.carries_all(&item.path.to_string_lossy(), &tags)
            })
            .map(|item| self.library_row(item, searching))
            .filter(|row| self.item_passes(selection, builtin, row))
            .collect();
        rows.sort_by(|a, b| listing::natural_cmp(&a.name, &b.name));
        Some(rows)
    }
}
