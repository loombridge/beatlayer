//! Collections (spec: browser-tags R34 to R40): the seven colours any item
//! of the browser can be in, kept in the database, and the labels that list
//! their items.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use bl_shell_spec::browser::{
    AddCollectionArgs, ClearCollectionsArgs, DeleteCollectionArgs, Item, ItemKind, Label,
    MoveCollectionArgs, RevealArgs, SetCollectionArgs,
};

use bl_plugin_host::scan::ScannedPlugin;

use super::db::{CollectionOp, NestedRow};
use super::filter::{BuiltinIds, Selection};
use super::listing::{self, Entry};
use super::{BrowserError, BrowserView, device_by_id, plugin_key};

/// The first and last of the seven collections' numbers.
const NUMBERS: std::ops::RangeInclusive<u8> = 1..=7;

/// The name a collection made without one is given (spec: browser-tags
/// R45).
const NEW_COLLECTION: &str = "New Collection";

/// The key a nested collection's folder row has in the content pane, and
/// its fold in the sidebar.
pub const FOLDER_PREFIX: &str = "collection:";

/// The collections each item is in, by its key (spec: browser-tags R5), and
/// the collections of the person's own inside collections (R45). The seven
/// are numbered 1 to 7; a nested one 8 and up.
#[derive(Clone, Debug, Default)]
pub struct CollectionStore {
    items: BTreeMap<String, BTreeSet<u32>>,
    /// The nested collections, in the order they are listed.
    nested: Vec<NestedRow>,
}

impl CollectionStore {
    /// Takes what the database holds.
    pub fn load(&mut self, rows: Vec<(String, Vec<u32>)>, nested: Vec<NestedRow>) {
        // A damaged file can hold a loop or a parent that isn't there: keep
        // the collections whose chain reaches one of the seven.
        let reaches_top = |row: &NestedRow| {
            let mut at = row.parent;
            for _ in 0..nested.len() {
                if u8::try_from(at).is_ok_and(|n| NUMBERS.contains(&n)) {
                    return true;
                }
                match nested.iter().find(|n| n.id == at) {
                    Some(parent) => at = parent.parent,
                    None => return false,
                }
            }
            false
        };
        self.nested = nested
            .iter()
            .filter(|row| reaches_top(row))
            .cloned()
            .collect();
        self.items = rows
            .into_iter()
            .map(|(key, collections)| {
                let numbers = collections.into_iter().filter(|n| self.exists(*n));
                (key, numbers.collect::<BTreeSet<u32>>())
            })
            .filter(|(_, numbers)| !numbers.is_empty())
            .collect();
    }

    /// Whether `number` names one of the seven or a nested collection.
    #[must_use]
    pub fn exists(&self, number: u32) -> bool {
        u8::try_from(number).is_ok_and(|n| NUMBERS.contains(&n))
            || self.nested.iter().any(|n| n.id == number)
    }

    /// The nested collections, in order.
    #[must_use]
    pub fn nested(&self) -> &[NestedRow] {
        &self.nested
    }

    /// The name of a nested collection.
    #[must_use]
    pub fn name_of(&self, number: u32) -> Option<&str> {
        self.nested
            .iter()
            .find(|n| n.id == number)
            .map(|n| n.name.as_str())
    }

    /// The collections directly inside `parent`, in order.
    #[must_use]
    pub fn children(&self, parent: u32) -> Vec<u32> {
        self.nested
            .iter()
            .filter(|n| n.parent == parent)
            .map(|n| n.id)
            .collect()
    }

    /// The collections inside `number`, at any depth.
    #[must_use]
    pub fn descendants(&self, number: u32) -> Vec<u32> {
        let mut found = Vec::new();
        let mut queue = self.children(number);
        while let Some(next) = queue.pop() {
            found.push(next);
            queue.extend(self.children(next));
        }
        found
    }

    /// The collection at the top of `number`'s chain: itself for one of the
    /// seven, which gives a nested collection its colour (R46).
    #[must_use]
    pub fn top_of(&self, number: u32) -> u8 {
        let mut at = number;
        while let Some(parent) = self.nested.iter().find(|n| n.id == at).map(|n| n.parent) {
            at = parent;
        }
        u8::try_from(at).unwrap_or(0)
    }

    /// The collections an item is in, in order, nested ones included.
    #[must_use]
    pub fn of(&self, key: &str) -> Vec<u32> {
        self.items
            .get(key)
            .map_or_else(Vec::new, |numbers| numbers.iter().copied().collect())
    }

    /// The seven collections' colours a row shows for an item: the ones it
    /// is in, and the top collection of each nested one (R46), in order.
    #[must_use]
    pub fn colours_of(&self, key: &str) -> Vec<u8> {
        let tops: BTreeSet<u8> = self.of(key).into_iter().map(|n| self.top_of(n)).collect();
        tops.into_iter().collect()
    }

    /// The nested collections an item is directly in.
    #[must_use]
    pub fn nested_of(&self, key: &str) -> Vec<u32> {
        self.of(key).into_iter().filter(|n| *n > 7).collect()
    }

    /// Whether an item is in a collection.
    #[must_use]
    pub fn contains(&self, key: &str, number: u32) -> bool {
        self.items.get(key).is_some_and(|n| n.contains(&number))
    }

    /// The keys of the items in a collection, in order.
    #[must_use]
    pub fn members(&self, number: u32) -> Vec<String> {
        self.items
            .iter()
            .filter(|(_, numbers)| numbers.contains(&number))
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// The keys of the items in a collection or in any collection inside it
    /// (R47), in order.
    #[must_use]
    pub fn members_deep(&self, number: u32) -> Vec<String> {
        let mut within = self.descendants(number);
        within.push(number);
        self.items
            .iter()
            .filter(|(_, numbers)| within.iter().any(|n| numbers.contains(n)))
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Puts an item in exactly these collections, and says what to write to
    /// the database when that changes anything.
    pub fn set(&mut self, key: &str, numbers: BTreeSet<u32>) -> Option<CollectionOp> {
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

    /// Makes a collection inside `parent`, named `name`, or "New
    /// Collection" numbered when that is taken there; its number.
    fn add(&mut self, parent: u32, name: Option<String>) -> u32 {
        let id = self.nested.iter().map(|n| n.id).max().unwrap_or(7).max(7) + 1;
        let taken = |name: &str| {
            self.nested
                .iter()
                .any(|n| n.parent == parent && n.name == name)
        };
        let name = name.unwrap_or_else(|| {
            let mut count = 1;
            let mut name = NEW_COLLECTION.to_owned();
            while taken(&name) {
                count += 1;
                name = format!("{NEW_COLLECTION} {count}");
            }
            name
        });
        self.nested.push(NestedRow { id, parent, name });
        id
    }

    /// Removes nested collections.
    fn remove(&mut self, gone: &[u32]) {
        self.nested.retain(|n| !gone.contains(&n.id));
    }

    /// Moves a nested collection into `parent`, before the `index`th
    /// collection there (the end if `None`).
    fn move_to(&mut self, id: u32, parent: u32, index: Option<usize>) {
        let Some(at) = self.nested.iter().position(|n| n.id == id) else {
            return;
        };
        let mut moved = self.nested.remove(at);
        moved.parent = parent;
        let siblings: Vec<usize> = self
            .nested
            .iter()
            .enumerate()
            .filter(|(_, n)| n.parent == parent)
            .map(|(i, _)| i)
            .collect();
        let place = match index.and_then(|i| siblings.get(i)) {
            Some(&before) => before,
            None => siblings.last().map_or(self.nested.len(), |last| last + 1),
        };
        self.nested.insert(place, moved);
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
        if let Some(key) = keys.iter().find(|key| key.starts_with(FOLDER_PREFIX)) {
            return Err(BrowserError::Unavailable(format!(
                "{key} is a collection, which a collection can't hold"
            )));
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
        self.check_collection(collection)?;
        let keys = self.collection_targets(items)?;
        let put = member.unwrap_or_else(|| {
            !keys
                .iter()
                .all(|key| self.collections.contains(key, collection))
        });
        let ops = keys
            .iter()
            .filter_map(|key| {
                let mut numbers: BTreeSet<u32> = self.collections.of(key).into_iter().collect();
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
        let label = Label::Collection(u32::from(self.collections.top_of(collection)));
        let comes_back = put && !self.label_prefs.shown(&label);
        if comes_back {
            self.label_prefs.set_shown(&label, true);
        }
        self.rebuild();
        Ok(comes_back)
    }

    /// Whether `number` names a collection the browser has.
    pub(super) fn check_collection(&self, number: u32) -> Result<(), BrowserError> {
        if self.collections.exists(number) {
            return Ok(());
        }
        Err(BrowserError::InvalidArgs(format!(
            "{number} isn't a collection: they are 1 (Favorites) to 7 (Gray), and the \
             numbers `browser.state` lists under them"
        )))
    }

    /// Writes the nested collections to the database.
    fn save_nested(&mut self) {
        self.generation += 1;
        if let Some(db) = &self.db {
            db.save_nested_collections(self.collections.nested().to_vec());
        }
    }

    /// Makes a collection inside another (spec: browser-tags R45), and
    /// opens its parent so it shows.
    pub(super) fn add_collection(&mut self, args: AddCollectionArgs) -> Result<(), BrowserError> {
        let AddCollectionArgs { parent, name } = args;
        self.check_collection(parent)?;
        let name = name.map(|name| name.trim().to_owned());
        if name.as_deref() == Some("") {
            return Err(BrowserError::InvalidArgs(
                "a collection needs a name".to_owned(),
            ));
        }
        let editing = name.is_none();
        let id = self.collections.add(parent, name);
        self.save_nested();
        self.open
            .insert(PathBuf::from(format!("{FOLDER_PREFIX}{parent}")));
        if editing {
            self.renaming_label = Some(Label::Collection(id));
        }
        self.rebuild();
        Ok(())
    }

    /// Renames a nested collection (spec: browser-tags R47).
    pub(super) fn rename_nested(&mut self, number: u32, name: String) -> Result<(), BrowserError> {
        if name.is_empty() {
            return Err(BrowserError::InvalidArgs(
                "a collection needs a name".to_owned(),
            ));
        }
        if let Some(row) = self.collections.nested.iter_mut().find(|n| n.id == number) {
            row.name = name;
        }
        self.save_nested();
        Ok(())
    }

    /// Moves a nested collection (spec: browser-tags R45).
    pub(super) fn move_collection(&mut self, args: MoveCollectionArgs) -> Result<(), BrowserError> {
        let MoveCollectionArgs {
            collection,
            parent,
            index,
        } = args;
        if collection <= 7 || !self.collections.exists(collection) {
            return Err(BrowserError::InvalidArgs(format!(
                "{collection} isn't a collection of your own: only those move"
            )));
        }
        self.check_collection(parent)?;
        if parent == collection || self.collections.descendants(collection).contains(&parent) {
            return Err(BrowserError::InvalidArgs(
                "a collection can't go inside itself or one of its own".to_owned(),
            ));
        }
        self.collections.move_to(collection, parent, index);
        self.save_nested();
        self.open
            .insert(PathBuf::from(format!("{FOLDER_PREFIX}{parent}")));
        self.rebuild();
        Ok(())
    }

    /// How many items a nested collection and the ones inside it hold, as
    /// Delete in its context menu asks about (spec: browser-tags R47).
    #[must_use]
    pub fn collection_holds(&self, number: u32) -> usize {
        self.collections.members_deep(number).len()
    }

    /// Deletes a nested collection and the ones inside it (spec:
    /// browser-tags R47); no file is touched.
    pub(super) fn delete_collection(
        &mut self,
        args: DeleteCollectionArgs,
    ) -> Result<(), BrowserError> {
        let DeleteCollectionArgs {
            collection,
            confirm,
        } = args;
        if collection <= 7 || !self.collections.exists(collection) {
            return Err(BrowserError::InvalidArgs(format!(
                "{collection} isn't a collection of your own: the seven can't be deleted"
            )));
        }
        let held = self.collection_holds(collection);
        if held > 0 && !confirm {
            return Err(BrowserError::Unavailable(format!(
                "{} and the collections inside it hold {held} items, which leave them: run it \
                 again with `confirm` to delete it",
                self.label_name(&Label::Collection(collection))
            )));
        }
        let mut gone = self.collections.descendants(collection);
        gone.push(collection);
        let ops = self
            .collections
            .members_deep(collection)
            .into_iter()
            .filter_map(|key| {
                let mut numbers: BTreeSet<u32> = self.collections.of(&key).into_iter().collect();
                numbers.retain(|n| !gone.contains(n));
                self.collections.set(&key, numbers)
            })
            .collect();
        let parent = self
            .collections
            .nested()
            .iter()
            .find(|n| n.id == collection)
            .map_or(0, |n| n.parent);
        self.collections.remove(&gone);
        self.save_collections(ops);
        self.save_nested();
        for number in &gone {
            self.open
                .remove(&PathBuf::from(format!("{FOLDER_PREFIX}{number}")));
        }
        if let Label::Collection(chosen) = self.label
            && gone.contains(&chosen)
        {
            self.label = Label::Collection(parent);
        }
        if self.renaming_label == Some(Label::Collection(collection)) {
            self.renaming_label = None;
        }
        self.rebuild();
        Ok(())
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

    /// A collection's rows (spec: browser-tags R36, R47): the collections
    /// nested in it first, as folders that open in place, then the items in
    /// it that can be found, folders first, each group by name or the chosen
    /// sort, folders and sets open as they do under any label. A search
    /// lists the matching items alone, as it does elsewhere, and covers the
    /// nested collections too.
    pub(super) fn collection_rows(
        &mut self,
        number: u32,
        tags: &[i64],
        selection: &Selection,
        builtin: &BuiltinIds,
    ) -> Vec<Item> {
        let query = self.search.trim().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let filtering = !words.is_empty() || !tags.is_empty() || !selection.is_empty();
        let mut items: Vec<Item> = if filtering {
            let keys = self.collections.members_deep(number);
            self.collected_rows(keys, 0, true)
        } else {
            self.collection_level(number, 0)
        };
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

    /// One level of a collection's rows: its nested collections as folders,
    /// each followed by its own level while open, then its items.
    fn collection_level(&mut self, number: u32, depth: u16) -> Vec<Item> {
        let mut items = Vec::new();
        for child in self.collections.children(number) {
            let key = format!("{FOLDER_PREFIX}{child}");
            let open = self.open.contains(Path::new(&key));
            items.push(Item {
                path: key,
                name: self.label_name(&Label::Collection(child)),
                kind: ItemKind::Folder,
                depth,
                open,
                folder: None,
                format: None,
                clip_kind: None,
                details: None,
                collections: vec![self.collections.top_of(child)],
                hidden: false,
                nested: Vec::new(),
            });
            if open {
                items.extend(self.collection_level(child, depth.saturating_add(1)));
            }
        }
        let keys = self.collections.members(number);
        items.extend(self.collected_rows(keys, depth, false));
        items
    }

    /// The rows of the items `keys` name: files and folders (opening in place
    /// unless `flat`), then plug-ins and devices.
    fn collected_rows(&mut self, keys: Vec<String>, depth: u16, flat: bool) -> Vec<Item> {
        let mut entries: Vec<Entry> = Vec::new();
        let mut plugins: Vec<Item> = Vec::new();
        let by_key: HashMap<String, &ScannedPlugin> =
            self.plugins.iter().map(|p| (p.key(), p)).collect();
        for key in keys {
            if let Some(plugin) = by_key.get(&key) {
                plugins.push(Item {
                    path: plugin_key(plugin),
                    name: plugin.name.clone(),
                    kind: ItemKind::Plugin,
                    depth,
                    open: false,
                    folder: plugin.vendor.clone(),
                    format: Some(plugin.format),
                    clip_kind: None,
                    details: None,
                    collections: Vec::new(),
                    hidden: self.hidden_plugins.contains(&plugin.key()),
                    nested: Vec::new(),
                });
            } else if let Some(device) = device_by_id(&key) {
                plugins.push(Item {
                    path: device.id,
                    name: device.name,
                    kind: ItemKind::Device,
                    depth,
                    open: false,
                    folder: None,
                    format: None,
                    clip_kind: None,
                    details: None,
                    collections: Vec::new(),
                    hidden: false,
                    nested: Vec::new(),
                });
            } else if let Some(entry) = Self::collected_entry(&key) {
                entries.push(entry);
            }
        }
        entries.sort_by(listing::browser_order);
        self.sort_entries(&mut entries);
        let mut items: Vec<Item> = Vec::new();
        if flat {
            items.extend(entries.into_iter().map(|entry| Item {
                path: entry.path.to_string_lossy().into_owned(),
                name: entry.name,
                kind: entry.kind,
                depth,
                open: false,
                folder: None,
                format: None,
                clip_kind: entry.clip,
                details: None,
                collections: Vec::new(),
                hidden: false,
                nested: Vec::new(),
            }));
        } else {
            self.push_entries(entries, depth, &mut items);
        }
        items.extend(plugins);
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
        let numbers = |list: &[u32]| list.iter().copied().collect::<BTreeSet<u32>>();
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
        store.load(vec![("/a.wav".to_owned(), vec![0, 2, 9])], Vec::new());
        assert_eq!(store.of("/a.wav"), [2]);
    }

    // Spec: browser-tags R45. A loop or an orphan in a damaged file is left
    // out, so walking the collections always ends.
    #[test]
    fn a_damaged_file_with_a_loop_or_an_orphan_loads_without_them() {
        let row = |id, parent| NestedRow {
            id,
            parent,
            name: format!("c{id}"),
        };
        let mut store = CollectionStore::default();
        store.load(
            vec![("/a.wav".to_owned(), vec![8, 9, 10, 11])],
            vec![row(8, 2), row(9, 10), row(10, 9), row(11, 40)],
        );
        assert_eq!(store.nested().len(), 1);
        assert_eq!(store.of("/a.wav"), [8]);
        assert_eq!(store.top_of(8), 2);
    }

    // Spec: browser-tags R45, R46. A nested collection is numbered from 8,
    // is named "New Collection" and numbered when taken, takes the colour of
    // the collection at its top, and moves before a sibling.
    #[test]
    fn nested_collections_are_numbered_named_coloured_and_ordered() {
        let mut store = CollectionStore::default();
        let a = store.add(2, None);
        let b = store.add(2, None);
        let c = store.add(a, Some("Deep".to_owned()));
        assert_eq!([a, b, c], [8, 9, 10]);
        assert_eq!(store.name_of(b), Some("New Collection 2"));
        assert_eq!(store.top_of(c), 2);
        assert_eq!(store.top_of(5), 5);
        assert_eq!(store.descendants(2).len(), 3);
        store.move_to(b, 2, Some(0));
        assert_eq!(store.children(2), [b, a]);
        store.move_to(c, 2, None);
        assert_eq!(store.children(2), [b, a, c]);
        assert!(store.exists(8) && !store.exists(11) && !store.exists(0));
    }
}
