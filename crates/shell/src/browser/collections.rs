//! Collections (spec: browser-tags R34 to R40): the seven colours any item
//! of the browser can be in, kept in the database, and the labels that list
//! their items.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use bl_shell_spec::browser::{
    ClearCollectionsArgs, Item, ItemKind, Label, RevealArgs, SetCollectionArgs,
};

use bl_plugin_host::scan::ScannedPlugin;

use super::db::CollectionOp;
use super::filter::{BuiltinIds, Selection};
use super::listing::{self, Entry};
use super::{BrowserError, BrowserView, device_by_id, plugin_key};

/// The first and last collection's numbers.
const NUMBERS: std::ops::RangeInclusive<u8> = 1..=7;

/// The collections each item is in, by its key (spec: browser-tags R5).
#[derive(Clone, Debug, Default)]
pub struct CollectionStore {
    items: BTreeMap<String, BTreeSet<u8>>,
}

impl CollectionStore {
    /// Takes what the database holds.
    pub fn load(&mut self, rows: Vec<(String, Vec<u8>)>) {
        self.items = rows
            .into_iter()
            .map(|(key, collections)| {
                let numbers = collections.into_iter().filter(|n| NUMBERS.contains(n));
                (key, numbers.collect::<BTreeSet<u8>>())
            })
            .filter(|(_, numbers)| !numbers.is_empty())
            .collect();
    }

    /// The collections an item is in, in order.
    #[must_use]
    pub fn of(&self, key: &str) -> Vec<u8> {
        self.items
            .get(key)
            .map_or_else(Vec::new, |numbers| numbers.iter().copied().collect())
    }

    /// Whether an item is in a collection.
    #[must_use]
    pub fn contains(&self, key: &str, number: u8) -> bool {
        self.items.get(key).is_some_and(|n| n.contains(&number))
    }

    /// The keys of the items in a collection, in order.
    #[must_use]
    pub fn members(&self, number: u8) -> Vec<String> {
        self.items
            .iter()
            .filter(|(_, numbers)| numbers.contains(&number))
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Puts an item in exactly these collections, and says what to write to
    /// the database when that changes anything.
    pub fn set(&mut self, key: &str, numbers: BTreeSet<u8>) -> Option<CollectionOp> {
        let before = self.items.get(key).cloned().unwrap_or_default();
        if before == numbers {
            return None;
        }
        let collections = numbers.iter().copied().collect();
        if numbers.is_empty() {
            self.items.remove(key);
        } else {
            self.items.insert(key.to_owned(), numbers);
        }
        Some(CollectionOp {
            key: key.to_owned(),
            collections,
        })
    }
}

impl BrowserView {
    /// The keys of the items a collection command acts on: the ones named,
    /// or the selected ones.
    fn collection_targets(&self, items: Option<Vec<String>>) -> Result<Vec<String>, BrowserError> {
        let keys: Vec<String> = match items {
            Some(items) => items.iter().map(|item| self.tag_key(item)).collect(),
            None => self.selected_keys(),
        };
        if keys.is_empty() {
            return Err(BrowserError::Unavailable(
                "no items are selected".to_owned(),
            ));
        }
        if let Some(key) = keys.iter().find(|key| self.is_set_row(key)) {
            return Err(BrowserError::Unavailable(format!(
                "{key} is a row of an opened set, which a collection can't hold"
            )));
        }
        Ok(keys)
    }

    /// Whether `browser.set_collection` and `browser.clear_collections` have
    /// items to act on.
    pub(super) fn can_collect(&self, items: Option<&[String]>) -> bool {
        items.map_or(!self.selected.is_empty(), |items| !items.is_empty())
    }

    /// Writes the changes to the collections to the database.
    pub(super) fn save_collections(&mut self, ops: Vec<CollectionOp>) {
        if ops.is_empty() {
            return;
        }
        self.generation += 1;
        if let Some(db) = &self.db {
            db.save_collections(ops);
        }
    }

    /// Puts items in a collection, or takes them out (spec: browser-tags
    /// R35); whether the preferences changed, because a hidden collection
    /// came back (R39).
    pub(super) fn set_collection(&mut self, args: SetCollectionArgs) -> Result<bool, BrowserError> {
        let SetCollectionArgs {
            collection,
            items,
            member,
        } = args;
        if !NUMBERS.contains(&collection) {
            return Err(BrowserError::InvalidArgs(format!(
                "{collection} isn't a collection: they are 1 (Favorites) to 7 (Gray)"
            )));
        }
        let keys = self.collection_targets(items)?;
        let put = member.unwrap_or_else(|| {
            !keys
                .iter()
                .all(|key| self.collections.contains(key, collection))
        });
        let ops = keys
            .iter()
            .filter_map(|key| {
                let mut numbers: BTreeSet<u8> = self.collections.of(key).into_iter().collect();
                if put {
                    numbers.insert(collection);
                } else {
                    numbers.remove(&collection);
                }
                self.collections.set(key, numbers)
            })
            .collect();
        self.save_collections(ops);
        self.hash_tagged_files(&keys);
        let label = Label::Collection(collection);
        let comes_back = put && !self.label_prefs.shown(&label);
        if comes_back {
            self.label_prefs.set_shown(&label, true);
        }
        self.rebuild();
        Ok(comes_back)
    }

    /// Takes items out of every collection (spec: browser-tags R35).
    pub(super) fn clear_collections(
        &mut self,
        args: ClearCollectionsArgs,
    ) -> Result<(), BrowserError> {
        let keys = self.collection_targets(args.items)?;
        let ops = keys
            .iter()
            .filter_map(|key| self.collections.set(key, BTreeSet::new()))
            .collect();
        self.save_collections(ops);
        self.rebuild();
        Ok(())
    }

    /// The paths Show in Finder shows: the ones named, or the selected
    /// items, which must be files or folders.
    pub(super) fn reveal_targets(&self, args: RevealArgs) -> Result<Vec<PathBuf>, BrowserError> {
        let paths: Vec<PathBuf> = match args.paths {
            Some(paths) => paths.iter().map(PathBuf::from).collect(),
            None => self.selected.clone(),
        };
        if paths.is_empty() {
            return Err(BrowserError::Unavailable(
                "no items are selected".to_owned(),
            ));
        }
        if let Some(path) = paths.iter().find(|path| !path.exists()) {
            return Err(BrowserError::InvalidArgs(format!(
                "{} isn't a file or folder the file manager can show",
                path.to_string_lossy()
            )));
        }
        Ok(paths)
    }

    /// A collection's rows (spec: browser-tags R36): the items in it that
    /// can be found, folders first, each group by name or the chosen sort,
    /// folders and sets open as they do under any label. A search lists the
    /// matching items alone, as it does elsewhere.
    pub(super) fn collection_rows(
        &mut self,
        number: u8,
        tags: &[i64],
        selection: &Selection,
        builtin: &BuiltinIds,
    ) -> Vec<Item> {
        let query = self.search.trim().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let filtering = !words.is_empty() || !tags.is_empty() || !selection.is_empty();
        let mut entries: Vec<Entry> = Vec::new();
        let mut plugins: Vec<Item> = Vec::new();
        let by_key: HashMap<String, &ScannedPlugin> =
            self.plugins.iter().map(|p| (p.key(), p)).collect();
        for key in self.collections.members(number) {
            if let Some(plugin) = by_key.get(&key) {
                plugins.push(Item {
                    path: plugin_key(plugin),
                    name: plugin.name.clone(),
                    kind: ItemKind::Plugin,
                    depth: 0,
                    open: false,
                    folder: plugin.vendor.clone(),
                    format: Some(plugin.format),
                    clip_kind: None,
                    details: None,
                    collections: Vec::new(),
                    hidden: self.hidden_plugins.contains(&plugin.key()),
                });
            } else if let Some(device) = device_by_id(&key) {
                plugins.push(Item {
                    path: device.id,
                    name: device.name,
                    kind: ItemKind::Device,
                    depth: 0,
                    open: false,
                    folder: None,
                    format: None,
                    clip_kind: None,
                    details: None,
                    collections: Vec::new(),
                    hidden: false,
                });
            } else if let Some(entry) = Self::collected_entry(&key) {
                entries.push(entry);
            }
        }
        entries.sort_by(listing::browser_order);
        self.sort_entries(&mut entries);
        let mut items: Vec<Item> = Vec::new();
        if filtering {
            items.extend(entries.into_iter().map(|entry| Item {
                path: entry.path.to_string_lossy().into_owned(),
                name: entry.name,
                kind: entry.kind,
                depth: 0,
                open: false,
                folder: None,
                format: None,
                clip_kind: entry.clip,
                details: None,
                collections: Vec::new(),
                hidden: false,
            }));
        } else {
            self.push_entries(entries, 0, &mut items);
        }
        items.extend(plugins);
        if filtering {
            items.retain(|item| {
                let name = item.name.to_lowercase();
                words.iter().all(|word| name.contains(word))
                    && self.carries_all(&self.tag_key(&item.path), tags)
                    && self.item_passes(selection, builtin, item)
            });
        }
        if items.is_empty() {
            self.note = Some(if !filtering {
                format!(
                    "Nothing is in {} yet. Select items and press {number}.",
                    self.label_name(&Label::Collection(number))
                )
            } else if query.is_empty() {
                "Nothing in this collection carries the selected tags.".to_owned()
            } else {
                format!("Nothing matches \u{201c}{}\u{201d}.", self.search.trim())
            });
        }
        items
    }

    /// The entry a collected file or folder lists as, or `None` when it
    /// isn't there or isn't a kind the browser lists.
    pub(super) fn collected_entry(key: &str) -> Option<Entry> {
        let path = Path::new(key);
        let name = path.file_name()?.to_string_lossy().into_owned();
        if listing::is_hidden(&name) {
            return None;
        }
        let kind = if path.is_dir() {
            ItemKind::Folder
        } else if path.is_file() {
            listing::file_kind(&name)?
        } else {
            return None;
        };
        let clip = (kind == ItemKind::Clip)
            .then(|| listing::clip_kind(path))
            .flatten();
        Some(Entry {
            path: path.to_path_buf(),
            name,
            kind,
            clip,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: browser-tags R35.
    #[test]
    fn an_item_is_in_several_collections_and_leaves_them_all_at_once() {
        let mut store = CollectionStore::default();
        let numbers = |list: &[u8]| list.iter().copied().collect::<BTreeSet<u8>>();
        assert!(store.set("/a.wav", numbers(&[3, 1])).is_some());
        assert_eq!(store.of("/a.wav"), [1, 3]);
        assert!(store.set("/a.wav", numbers(&[1, 3])).is_none(), "no change");
        assert_eq!(store.members(3), ["/a.wav"]);
        let op = store.set("/a.wav", BTreeSet::new()).expect("a change");
        assert!(op.collections.is_empty());
        assert!(store.of("/a.wav").is_empty() && store.members(1).is_empty());
    }

    #[test]
    fn what_the_database_holds_outside_one_to_seven_is_left_out() {
        let mut store = CollectionStore::default();
        store.load(vec![("/a.wav".to_owned(), vec![0, 2, 9])]);
        assert_eq!(store.of("/a.wav"), [2]);
    }
}
