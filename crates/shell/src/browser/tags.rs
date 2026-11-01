//! The browser's tags: the groups, the tags in them, and the items that
//! carry them (spec: browser-tags R6 to R9).
//!
//! Every tag belongs to one group, and a tag can hold subtags, one level
//! deep. A tag's full name is its path: the group's name, the tag's and any
//! subtag's, joined by `|`, as `Format|CLAP`.
//!
//! Beatlayer fills four groups of its own — Content, Format, Creator and
//! Devices — from what it knows of each item. Their tags are in the
//! catalogue like any other's, but they can't be renamed or deleted, and
//! they can't be taken off an item. A fifth, Function, holds automatic
//! tags: Beatlayer gives them from a plug-in's categories and a sample's
//! sound, keeps them apart from a person's, and remembers the ones a person
//! took off (spec: browser-tags R49 to R54). The rest are the person's.

use std::collections::{BTreeMap, BTreeSet};

use super::db::TagOp;

/// The groups Beatlayer fills itself, in order (spec: browser-tags R7).
pub const BUILTIN_GROUPS: [&str; 4] = ["Content", "Format", "Creator", "Devices"];

/// The tags Beatlayer gives in the Content group (spec: browser-tags R7).
pub const CONTENT_TAGS: [&str; 8] = [
    "Sample",
    "Audio Clip",
    "MIDI Clip",
    "Preset",
    "Set",
    "Backup Set",
    "Device",
    "Plug-In",
];

/// The tags Beatlayer gives in the Format group (spec: browser-tags R7).
pub const FORMAT_TAGS: [&str; 3] = ["CLAP", "VST3", "AUv2"];

/// The tags Beatlayer gives in the Devices group (spec: browser-tags R7).
pub const DEVICE_TAGS: [&str; 3] = ["Instrument", "Audio Effect", "MIDI Effect"];

/// The Function group, which holds the automatic tags (spec: browser-tags
/// R49).
pub const FUNCTION_GROUP: &str = "Function";

/// The tags in the Function group, in order: instruments' first, then
/// effects', then the ones only samples have; Bass, Synth and Percussion
/// serve instruments and samples alike (spec: browser-tags R50).
/// Beatlayer's own list.
pub const FUNCTION_TAGS: [&str; 34] = [
    "Synth",
    "Bass",
    "Keys",
    "Piano",
    "Organ",
    "Strings",
    "Brass",
    "Woodwind",
    "Drums",
    "Percussion",
    "Sampler",
    "EQ",
    "Filter",
    "Dynamics",
    "Distortion",
    "Delay",
    "Reverb",
    "Modulation",
    "Pitch",
    "Spatial",
    "Utility",
    "Analyser",
    "Kick",
    "Snare",
    "Clap",
    "Hi-Hat",
    "Cymbal",
    "Tom",
    "Pad",
    "Lead",
    "Vocal",
    "FX",
    "Loop",
    "One-Shot",
];

/// A tag group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagGroup {
    /// Its ID, which the database keeps.
    pub id: i64,
    /// Its name, as the Tag Editor and a tag's full name show it.
    pub name: String,
    /// Where it sits in the list.
    pub position: i32,
    /// Whether Beatlayer gave it: it can't be renamed or deleted.
    pub builtin: bool,
    /// Where it sat when it was made, which Reset Order puts it back to.
    pub first: i32,
}

/// A tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    /// Its ID, which the database keeps.
    pub id: i64,
    /// The group it's in.
    pub group: i64,
    /// Its own name, without the group's.
    pub name: String,
    /// The tag it sits under, for a subtag. A subtag holds none.
    pub parent: Option<i64>,
    /// Where it sits in its group.
    pub position: i32,
    /// Whether Beatlayer gave it: it can't be renamed or deleted, and it
    /// can't be taken off an item.
    pub builtin: bool,
    /// Where it sat when it was made, which Reset Order puts it back to.
    pub first: i32,
}

/// Where a tag on an item comes from (spec: browser-tags R16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The item carries it in its own right.
    Own,
    /// A folder it lies in carries it (spec: browser-tags R9).
    Folder,
    /// Beatlayer gave it, from what it knows of the item (R7).
    Beatlayer,
    /// Beatlayer gave it automatically, from a plug-in's categories or a
    /// sample's sound, and a person can take it off (spec: browser-tags
    /// R49 and R51).
    Auto,
}

impl Source {
    /// The source's name, as the agent API reports it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Own => "own",
            Self::Folder => "folder",
            Self::Beatlayer => "beatlayer",
            Self::Auto => "auto",
        }
    }
}

/// What an automatic tag comes from (spec: browser-tags R49 and R51).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AutoSource {
    /// A plug-in's declared categories.
    Plugin,
    /// A sample's sound.
    Sound,
}

impl AutoSource {
    /// The name the database keeps it under.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Plugin => "plugin",
            Self::Sound => "sound",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "plugin" => Some(Self::Plugin),
            "sound" => Some(Self::Sound),
            _ => None,
        }
    }
}

/// A tag on an item, and where it comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarriedTag {
    /// The tag's ID.
    pub tag: i64,
    /// Where it comes from.
    pub source: Source,
}

/// The browser's tags: the groups, the tags in them, and the items that
/// carry them.
#[derive(Clone, Debug, Default)]
pub struct TagStore {
    groups: Vec<TagGroup>,
    tags: Vec<Tag>,
    /// The tags each item carries in its own right, by its key.
    items: BTreeMap<String, BTreeSet<i64>>,
    /// The automatic tags each item has, and what each comes from
    /// (spec: browser-tags R51).
    auto: BTreeMap<String, BTreeMap<i64, AutoSource>>,
    /// The automatic tags a person took off each item: a rescan or a new
    /// analysis doesn't give them again (spec: browser-tags R51).
    removed: BTreeMap<String, BTreeSet<i64>>,
    /// The tags searched recently, most recent first.
    recent: Vec<i64>,
    next_id: i64,
}

impl TagStore {
    /// A store with Beatlayer's four groups and the tags it gives.
    #[must_use]
    pub fn new() -> Self {
        let mut store = Self::default();
        for (position, name) in BUILTIN_GROUPS.iter().enumerate() {
            let id = store.next_id;
            store.next_id += 1;
            store.groups.push(TagGroup {
                id,
                name: (*name).to_owned(),
                position: i32::try_from(position).unwrap_or(i32::MAX),
                builtin: true,
                first: i32::try_from(position).unwrap_or(i32::MAX),
            });
        }
        for (group, names) in [
            ("Content", &CONTENT_TAGS[..]),
            ("Format", &FORMAT_TAGS[..]),
            ("Devices", &DEVICE_TAGS[..]),
        ] {
            let Some(group_id) = store.group_id(group) else {
                continue;
            };
            for (position, name) in names.iter().enumerate() {
                let id = store.next_id;
                store.next_id += 1;
                store.tags.push(Tag {
                    id,
                    group: group_id,
                    name: (*name).to_owned(),
                    parent: None,
                    position: i32::try_from(position).unwrap_or(i32::MAX),
                    builtin: true,
                    first: i32::try_from(position).unwrap_or(i32::MAX),
                });
            }
        }
        store.ensure_function();
        store
    }

    /// Makes the Function group and its tags if they aren't there (spec:
    /// browser-tags R49 and R50). They are made after the others, with
    /// the next free IDs, so a database from before them has no clash.
    fn ensure_function(&mut self) {
        let group = if let Some(id) = self.group_id(FUNCTION_GROUP) {
            id
        } else {
            let id = self.next_id;
            self.next_id += 1;
            let position = i32::try_from(self.groups.len()).unwrap_or(i32::MAX);
            self.groups.push(TagGroup {
                id,
                name: FUNCTION_GROUP.to_owned(),
                position,
                builtin: true,
                first: position,
            });
            id
        };
        for name in FUNCTION_TAGS {
            if !self.tags.iter().any(|t| t.group == group && t.name == name) {
                // The database learns of the tag when `load` writes what it
                // doesn't hold.
                let _ = self.add_builtin_tag(name, group);
            }
        }
    }

    /// Takes the Function group and its tags out of the store, before the
    /// database's own are read in.
    fn drop_function(&mut self) {
        if let Some(group) = self.group_id(FUNCTION_GROUP) {
            self.tags.retain(|t| t.group != group);
            self.groups.retain(|g| g.id != group);
        }
    }

    /// The next free ID, past every group and tag the store holds.
    fn recount(&mut self) {
        self.next_id = self
            .groups
            .iter()
            .map(|g| g.id)
            .chain(self.tags.iter().map(|t| t.id))
            .max()
            .unwrap_or(0)
            + 1;
    }

    /// The groups, in the order the Tag Editor shows them.
    #[must_use]
    pub fn groups(&self) -> Vec<&TagGroup> {
        let mut groups: Vec<&TagGroup> = self.groups.iter().collect();
        groups.sort_by_key(|g| (g.position, g.id));
        groups
    }

    /// A group by its ID.
    #[must_use]
    pub fn group(&self, id: i64) -> Option<&TagGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// A group's ID by its name, ignoring case.
    #[must_use]
    pub fn group_id(&self, name: &str) -> Option<i64> {
        self.groups
            .iter()
            .find(|g| g.name.eq_ignore_ascii_case(name))
            .map(|g| g.id)
    }

    /// A tag by its ID.
    #[must_use]
    pub fn tag(&self, id: i64) -> Option<&Tag> {
        self.tags.iter().find(|t| t.id == id)
    }

    /// The tags in a group, in order, without their subtags.
    #[must_use]
    pub fn tags_in(&self, group: i64) -> Vec<&Tag> {
        let mut tags: Vec<&Tag> = self
            .tags
            .iter()
            .filter(|t| t.group == group && t.parent.is_none())
            .collect();
        tags.sort_by_key(|t| (t.position, t.id));
        tags
    }

    /// The subtags of a tag, in order.
    #[must_use]
    pub fn subtags(&self, tag: i64) -> Vec<&Tag> {
        let mut tags: Vec<&Tag> = self.tags.iter().filter(|t| t.parent == Some(tag)).collect();
        tags.sort_by_key(|t| (t.position, t.id));
        tags
    }

    /// Every tag, in the order the Tag Editor shows them: group by group,
    /// and in each group the tags and their subtags.
    #[must_use]
    pub fn all_tags(&self) -> Vec<&Tag> {
        let mut tags: Vec<&Tag> = Vec::new();
        for group in self.groups() {
            for tag in self.tags_in(group.id) {
                tags.push(tag);
                for sub in self.subtags(tag.id) {
                    tags.push(sub);
                }
            }
        }
        tags
    }

    /// A tag's full name: its path, the group's name, the tag's and any
    /// subtag's, joined by `|`.
    #[must_use]
    pub fn full_name(&self, id: i64) -> String {
        let Some(tag) = self.tag(id) else {
            return String::new();
        };
        let group = self
            .group(tag.group)
            .map_or_else(String::new, |g| g.name.clone());
        match tag.parent.and_then(|parent| self.tag(parent)) {
            Some(parent) => format!("{group}|{}|{}", parent.name, tag.name),
            None => format!("{group}|{}", tag.name),
        }
    }

    /// The tags a name names: those whose full name is it, or, for a name
    /// with no `|` in it, those with that name in any group.
    #[must_use]
    pub fn find(&self, name: &str) -> Vec<i64> {
        let name = name.trim();
        if name.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<i64> = self
            .tags
            .iter()
            .filter(|t| self.full_name(t.id).eq_ignore_ascii_case(name))
            .map(|t| t.id)
            .collect();
        if found.is_empty() && !name.contains('|') {
            found = self
                .tags
                .iter()
                .filter(|t| t.name.eq_ignore_ascii_case(name))
                .map(|t| t.id)
                .collect();
        }
        found
    }

    /// The tags an item carries in its own right.
    #[must_use]
    pub fn item_tags(&self, key: &str) -> Vec<i64> {
        self.items
            .get(key)
            .map_or_else(Vec::new, |tags| tags.iter().copied().collect())
    }

    /// The tags an item carries in its own right, without copying them.
    #[must_use]
    pub fn item_tags_ref(&self, key: &str) -> Option<&BTreeSet<i64>> {
        self.items.get(key)
    }

    /// The items that carry a tag in their own right.
    #[must_use]
    pub fn items_with_tag(&self, tag: i64) -> Vec<String> {
        self.items
            .iter()
            .filter(|(_, tags)| tags.contains(&tag))
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// The automatic tags an item has, and what each comes from.
    #[must_use]
    pub fn auto_tags_ref(&self, key: &str) -> Option<&BTreeMap<i64, AutoSource>> {
        self.auto.get(key)
    }

    /// The items that have an automatic tag.
    #[must_use]
    pub fn items_with_auto_tag(&self, tag: i64) -> Vec<String> {
        self.auto
            .iter()
            .filter(|(_, tags)| tags.contains_key(&tag))
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// The automatic tags a person took off an item.
    #[must_use]
    pub fn removed_auto_tags(&self, key: &str) -> Vec<i64> {
        self.removed
            .get(key)
            .map_or_else(Vec::new, |tags| tags.iter().copied().collect())
    }

    /// Gives an item the automatic tags a source finds for it now, in place
    /// of the ones that source gave it before, and says what to write to
    /// the database. A tag the person took off isn't given again (spec:
    /// browser-tags R51).
    pub fn set_auto_tags(
        &mut self,
        key: &str,
        source: AutoSource,
        tags: impl IntoIterator<Item = i64>,
    ) -> Vec<TagOp> {
        let removed = self.removed.get(key);
        let wanted: BTreeSet<i64> = tags
            .into_iter()
            .filter(|tag| !removed.is_some_and(|removed| removed.contains(tag)))
            .collect();
        let entry = self.auto.entry(key.to_owned()).or_default();
        let before: BTreeSet<i64> = entry
            .iter()
            .filter(|(_, from)| **from == source)
            .map(|(tag, _)| *tag)
            .collect();
        if before == wanted {
            if entry.is_empty() {
                self.auto.remove(key);
            }
            return Vec::new();
        }
        entry.retain(|_, from| *from != source);
        for tag in &wanted {
            entry.insert(*tag, source);
        }
        if entry.is_empty() {
            self.auto.remove(key);
        }
        vec![TagOp::SetAutoTags {
            key: key.to_owned(),
            source: source.name().to_owned(),
            tags: wanted.into_iter().collect(),
        }]
    }

    /// Takes an automatic tag off an item and remembers it, so no rescan or
    /// analysis gives it again, and says what to write to the database
    /// (spec: browser-tags R51). Nothing happens if the item doesn't have
    /// it.
    pub fn remove_auto_tag(&mut self, key: &str, tag: i64) -> Vec<TagOp> {
        let Some(tags) = self.auto.get_mut(key) else {
            return Vec::new();
        };
        let Some(source) = tags.remove(&tag) else {
            return Vec::new();
        };
        let left: Vec<i64> = tags
            .iter()
            .filter(|(_, from)| **from == source)
            .map(|(tag, _)| *tag)
            .collect();
        if tags.is_empty() {
            self.auto.remove(key);
        }
        let removed = self.removed.entry(key.to_owned()).or_default();
        removed.insert(tag);
        vec![
            TagOp::SetAutoTags {
                key: key.to_owned(),
                source: source.name().to_owned(),
                tags: left,
            },
            TagOp::SetRemovedAutoTags {
                key: key.to_owned(),
                tags: removed.iter().copied().collect(),
            },
        ]
    }

    /// Passes what a file's automatic tags know to the path it moved to:
    /// the tags a person took off stay off (spec: browser-tags R5 and R51).
    pub fn move_removed_auto_tags(&mut self, old: &str, new: &str) -> Vec<TagOp> {
        let Some(moved) = self.removed.remove(old) else {
            return Vec::new();
        };
        let kept = self.removed.entry(new.to_owned()).or_default();
        kept.extend(moved);
        let mut ops = vec![
            TagOp::SetRemovedAutoTags {
                key: new.to_owned(),
                tags: kept.iter().copied().collect(),
            },
            TagOp::SetRemovedAutoTags {
                key: old.to_owned(),
                tags: Vec::new(),
            },
        ];
        // What the new path has already been given and the person had taken
        // off the old one goes.
        let gone: Vec<i64> = self.removed_auto_tags(new);
        for tag in gone {
            ops.extend(self.remove_auto_tag(new, tag));
        }
        ops
    }

    /// The tags searched recently, most recent first.
    #[must_use]
    pub fn recent(&self) -> &[i64] {
        &self.recent
    }

    /// Notes that a tag was searched, so it comes first among the
    /// suggestions next time.
    pub fn note_searched(&mut self, tag: i64) {
        self.recent.retain(|&id| id != tag);
        self.recent.insert(0, tag);
        self.recent.truncate(16);
    }

    /// Puts the tags on an item, or takes them off, and says what to write
    /// to the database.
    pub fn set_item_tags(&mut self, key: &str, tags: impl IntoIterator<Item = i64>) -> Vec<TagOp> {
        let tags: BTreeSet<i64> = tags.into_iter().collect();
        let before = self.items.get(key).cloned().unwrap_or_default();
        if before == tags {
            return Vec::new();
        }
        if tags.is_empty() {
            self.items.remove(key);
        } else {
            self.items.insert(key.to_owned(), tags.clone());
        }
        vec![TagOp::SetItemTags {
            key: key.to_owned(),
            tags: tags.into_iter().collect(),
        }]
    }

    /// Makes a tag group, and says what to write to the database.
    pub fn add_group(&mut self, name: &str) -> Result<(i64, Vec<TagOp>), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a tag group's name can't be empty".to_owned());
        }
        if self.group_id(name).is_some() {
            return Err(format!("there's already a group called {name}"));
        }
        let id = self.next_id;
        self.next_id += 1;
        let position = self.groups.len() as i32;
        let group = TagGroup {
            id,
            name: name.to_owned(),
            position,
            builtin: false,
            first: position,
        };
        self.groups.push(group.clone());
        Ok((
            id,
            vec![TagOp::AddGroup {
                id,
                name: name.to_owned(),
                position,
                builtin: false,
            }],
        ))
    }

    /// Makes a tag in a group, or a subtag under a tag, and says what to
    /// write to the database.
    pub fn add_tag(
        &mut self,
        name: &str,
        group: i64,
        parent: Option<i64>,
    ) -> Result<(i64, Vec<TagOp>), String> {
        self.make_tag(name, group, parent, false)
    }

    /// Makes a tag Beatlayer gives, such as a plug-in's vendor, and says
    /// what to write to the database.
    pub fn add_builtin_tag(&mut self, name: &str, group: i64) -> Result<(i64, Vec<TagOp>), String> {
        self.make_tag(name, group, None, true)
    }

    /// Makes a tag.
    fn make_tag(
        &mut self,
        name: &str,
        group: i64,
        parent: Option<i64>,
        builtin: bool,
    ) -> Result<(i64, Vec<TagOp>), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a tag's name can't be empty".to_owned());
        }
        if name.contains('|') {
            return Err("a tag's name can't hold a |".to_owned());
        }
        if let Some(parent_id) = parent {
            let Some(parent_tag) = self.tag(parent_id) else {
                return Err("there's no tag to put it under".to_owned());
            };
            if parent_tag.parent.is_some() {
                return Err("a subtag can't hold another subtag".to_owned());
            }
            if self
                .tags
                .iter()
                .any(|t| t.parent == Some(parent_id) && t.name.eq_ignore_ascii_case(name))
            {
                return Err(format!("there's already a subtag called {name}"));
            }
        } else if self
            .tags
            .iter()
            .any(|t| t.group == group && t.parent.is_none() && t.name.eq_ignore_ascii_case(name))
        {
            return Err(format!("there's already a tag called {name}"));
        }
        let id = self.next_id;
        self.next_id += 1;
        let position = self
            .tags
            .iter()
            .filter(|t| t.group == group && t.parent == parent)
            .count() as i32;
        let tag = Tag {
            id,
            group,
            name: name.to_owned(),
            parent,
            position,
            builtin,
            first: position,
        };
        self.tags.push(tag);
        Ok((
            id,
            vec![TagOp::AddTag {
                id,
                group,
                name: name.to_owned(),
                parent,
                position,
                builtin,
            }],
        ))
    }

    /// Renames a tag, and says what to write to the database. The tag keeps
    /// its ID, so every item that carries it keeps it.
    pub fn rename_tag(&mut self, id: i64, name: &str) -> Result<Vec<TagOp>, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a tag's name can't be empty".to_owned());
        }
        let Some(tag) = self.tags.iter().find(|t| t.id == id) else {
            return Err("there's no tag to rename".to_owned());
        };
        if tag.builtin {
            return Err(format!("{} is Beatlayer's own tag", tag.name));
        }
        if name.contains('|') {
            return Err("a tag's name can't hold a |".to_owned());
        }
        let clash = self.tags.iter().any(|t| {
            t.id != id
                && t.group == tag.group
                && t.parent == tag.parent
                && t.name.eq_ignore_ascii_case(name)
        });
        if clash {
            return Err(format!("there's already a tag called {name}"));
        }
        let Some(tag) = self.tags.iter_mut().find(|t| t.id == id) else {
            return Err("there's no tag to rename".to_owned());
        };
        tag.name = name.to_owned();
        Ok(vec![TagOp::RenameTag {
            id,
            name: name.to_owned(),
        }])
    }

    /// Renames a group, and says what to write to the database.
    pub fn rename_group(&mut self, id: i64, name: &str) -> Result<Vec<TagOp>, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a tag group's name can't be empty".to_owned());
        }
        let Some(group) = self.groups.iter().find(|g| g.id == id) else {
            return Err("there's no group to rename".to_owned());
        };
        if group.builtin {
            return Err(format!("{} is Beatlayer's own group", group.name));
        }
        if self.group_id(name).is_some_and(|other| other != id) {
            return Err(format!("there's already a group called {name}"));
        }
        let Some(group) = self.groups.iter_mut().find(|g| g.id == id) else {
            return Err("there's no group to rename".to_owned());
        };
        group.name = name.to_owned();
        Ok(vec![TagOp::RenameGroup {
            id,
            name: name.to_owned(),
        }])
    }

    /// Deletes a tag, taking it off every item, and says what to write to
    /// the database.
    pub fn delete_tag(&mut self, id: i64) -> Result<Vec<TagOp>, String> {
        let Some(tag) = self.tags.iter().find(|t| t.id == id) else {
            return Err("there's no tag to delete".to_owned());
        };
        if tag.builtin {
            return Err(format!("{} is Beatlayer's own tag", tag.name));
        }
        let subtags: Vec<i64> = self.subtags(id).iter().map(|t| t.id).collect();
        let mut ops = vec![TagOp::DeleteTag { id }];
        for subtag in subtags {
            self.tags.retain(|t| t.id != subtag);
            for tags in self.items.values_mut() {
                tags.remove(&subtag);
            }
            ops.push(TagOp::DeleteTag { id: subtag });
        }
        self.tags.retain(|t| t.id != id);
        for tags in self.items.values_mut() {
            tags.remove(&id);
        }
        self.recent.retain(|&recent| recent != id);
        Ok(ops)
    }

    /// Deletes a group with its tags, and says what to write to the
    /// database.
    pub fn delete_group(&mut self, id: i64) -> Result<Vec<TagOp>, String> {
        let Some(group) = self.groups.iter().find(|g| g.id == id) else {
            return Err("there's no group to delete".to_owned());
        };
        if group.builtin {
            return Err(format!("{} is Beatlayer's own group", group.name));
        }
        let mut ops = Vec::new();
        let tags: Vec<i64> = self
            .tags
            .iter()
            .filter(|t| t.group == id)
            .map(|t| t.id)
            .collect();
        for tag in tags {
            ops.extend(self.delete_tag(tag)?);
        }
        self.groups.retain(|g| g.id != id);
        ops.push(TagOp::DeleteGroup { id });
        Ok(ops)
    }

    /// Moves a tag to a new place in its group, and says what to write to
    /// the database.
    pub fn move_tag(&mut self, id: i64, position: i32) -> Result<Vec<TagOp>, String> {
        let Some(tag) = self.tags.iter().find(|t| t.id == id) else {
            return Err("there's no tag to move".to_owned());
        };
        if tag.builtin {
            return Err(format!("{} is Beatlayer's own tag", tag.name));
        }
        let group = tag.group;
        let parent = tag.parent;
        let mut siblings: Vec<i64> = self
            .tags
            .iter()
            .filter(|t| t.group == group && t.parent == parent && t.id != id)
            .map(|t| t.id)
            .collect();
        siblings.sort_by_key(|&other| {
            let tag = self.tag(other).expect("the tag is in the store");
            (tag.position, tag.id)
        });
        let position = position.clamp(0, siblings.len() as i32) as usize;
        siblings.insert(position, id);
        let mut ops = Vec::new();
        for (index, sibling) in siblings.iter().enumerate() {
            let tag = self.tags.iter_mut().find(|t| t.id == *sibling);
            if let Some(tag) = tag {
                tag.position = i32::try_from(index).unwrap_or(i32::MAX);
            }
            ops.push(TagOp::MoveTag {
                id: *sibling,
                position: i32::try_from(index).unwrap_or(i32::MAX),
            });
        }
        Ok(ops)
    }

    /// Moves a group to a new place in the list, and says what to write to
    /// the database.
    pub fn move_group(&mut self, id: i64, position: i32) -> Result<Vec<TagOp>, String> {
        let Some(group) = self.groups.iter().find(|g| g.id == id) else {
            return Err("there's no group to move".to_owned());
        };
        if group.builtin {
            return Err(format!("{} is Beatlayer's own group", group.name));
        }
        let mut siblings: Vec<i64> = self
            .groups
            .iter()
            .filter(|g| g.id != id)
            .map(|g| g.id)
            .collect();
        siblings.sort_by_key(|&other| {
            let group = self.group(other).expect("the group is in the store");
            (group.position, group.id)
        });
        let position = position.clamp(0, siblings.len() as i32) as usize;
        siblings.insert(position, id);
        let mut ops = Vec::new();
        for (index, sibling) in siblings.iter().enumerate() {
            if let Some(group) = self.groups.iter_mut().find(|g| g.id == *sibling) {
                group.position = i32::try_from(index).unwrap_or(i32::MAX);
            }
            ops.push(TagOp::MoveGroup {
                id: *sibling,
                position: i32::try_from(index).unwrap_or(i32::MAX),
            });
        }
        Ok(ops)
    }

    /// Puts every group and tag back in the order it was made in, and says
    /// what to write to the database.
    pub fn reset_order(&mut self) -> Vec<TagOp> {
        let mut ops = Vec::new();
        let mut groups: Vec<(i64, i32)> = self
            .groups
            .iter()
            .map(|group| (group.id, group.first))
            .collect();
        groups.sort_by_key(|(_, first)| *first);
        for (index, (id, _)) in groups.iter().enumerate() {
            let position = i32::try_from(index).unwrap_or(i32::MAX);
            if let Some(group) = self.groups.iter_mut().find(|g| g.id == *id) {
                group.position = position;
            }
            ops.push(TagOp::MoveGroup { id: *id, position });
        }
        let mut tags: Vec<(i64, i64, Option<i64>, i32)> = self
            .tags
            .iter()
            .map(|tag| (tag.id, tag.group, tag.parent, tag.first))
            .collect();
        tags.sort_by_key(|(_, group, parent, first)| (*group, *parent, *first));
        // Each tag's place is among the tags of its own group and parent.
        let mut places: BTreeMap<(i64, Option<i64>), i32> = BTreeMap::new();
        for (id, group, parent, _) in tags {
            let place = places.entry((group, parent)).or_insert(0);
            let position = *place;
            *place += 1;
            if let Some(tag) = self.tags.iter_mut().find(|t| t.id == id) {
                tag.position = position;
            }
            ops.push(TagOp::MoveTag { id, position });
        }
        ops
    }

    /// Adds the groups and tags the database holds and the store doesn't,
    /// so a second Beatlayer on the same configuration folder is known
    /// (spec: browser-tags R3). The store's own state wins.
    pub fn merge(&mut self, groups: Vec<TagGroup>, tags: Vec<Tag>) {
        for group in groups {
            if !self.groups.iter().any(|g| g.id == group.id) {
                self.groups.push(group);
            }
        }
        for tag in tags {
            if !self.tags.iter().any(|t| t.id == tag.id) {
                self.tags.push(tag);
            }
        }
        self.recount();
    }

    /// Fills the store from what the database holds, and says what to write
    /// to it: the groups and tags it doesn't have yet.
    ///
    /// The order the database holds wins: the person may have moved a
    /// group or a tag, and that order is kept between launches (spec:
    /// browser-tags R1 and R12). The order they were first made in is
    /// taken from their IDs, which grow as they are made, so Reset Order
    /// still knows it.
    pub fn load(
        &mut self,
        groups: Vec<TagGroup>,
        tags: Vec<Tag>,
        items: Vec<(String, Vec<i64>)>,
        auto: Vec<(String, i64, String)>,
        removed: Vec<(String, Vec<i64>)>,
    ) -> Vec<TagOp> {
        // The Function group the store made for itself has IDs of its own;
        // the database's, if it has one, wins.
        self.drop_function();
        let mut ops = Vec::new();
        let mut held: Vec<i64> = Vec::new();
        for group in groups {
            held.push(group.id);
            if let Some(existing) = self.groups.iter_mut().find(|g| g.id == group.id) {
                existing.position = group.position;
                continue;
            }
            self.groups.push(group.clone());
        }
        let mut held_tags: Vec<i64> = Vec::new();
        for tag in tags {
            held_tags.push(tag.id);
            if let Some(existing) = self.tags.iter_mut().find(|t| t.id == tag.id) {
                existing.position = tag.position;
                continue;
            }
            self.tags.push(tag.clone());
        }
        self.recount();
        self.ensure_function();
        // The groups and tags the file doesn't hold yet are written to it,
        // so it holds Beatlayer's own groups and the tags it gives too
        // (spec: browser-tags R1).
        for group in &self.groups {
            if !held.contains(&group.id) {
                ops.push(TagOp::AddGroup {
                    id: group.id,
                    name: group.name.clone(),
                    position: group.position,
                    builtin: group.builtin,
                });
            }
        }
        for tag in &self.tags {
            if !held_tags.contains(&tag.id) {
                ops.push(TagOp::AddTag {
                    id: tag.id,
                    group: tag.group,
                    name: tag.name.clone(),
                    parent: tag.parent,
                    position: tag.position,
                    builtin: tag.builtin,
                });
            }
        }
        for (key, tags) in items {
            let after: BTreeSet<i64> = tags.into_iter().collect();
            if after.is_empty() {
                self.items.remove(&key);
            } else {
                self.items.insert(key, after);
            }
        }
        self.recount();
        self.auto.clear();
        for (key, tag, source) in auto {
            if let Some(source) = AutoSource::from_name(&source) {
                self.auto.entry(key).or_default().insert(tag, source);
            }
        }
        self.removed = removed
            .into_iter()
            .map(|(key, tags)| (key, tags.into_iter().collect()))
            .collect();
        self.first_orders();
        ops
    }

    /// Puts each group and tag back in the order it was first made in: the
    /// order of its ID, which grows as it is made (spec: browser-tags R12).
    fn first_orders(&mut self) {
        let mut groups: Vec<i64> = self.groups.iter().map(|group| group.id).collect();
        groups.sort_unstable();
        for (index, id) in groups.into_iter().enumerate() {
            if let Some(group) = self.groups.iter_mut().find(|g| g.id == id) {
                group.first = i32::try_from(index).unwrap_or(i32::MAX);
            }
        }
        // Each tag's place is among the tags of its own group and parent.
        let mut tags: Vec<(i64, i64, Option<i64>)> = self
            .tags
            .iter()
            .map(|tag| (tag.id, tag.group, tag.parent))
            .collect();
        tags.sort_unstable();
        let mut places: BTreeMap<(i64, Option<i64>), i32> = BTreeMap::new();
        for (id, group, parent) in tags {
            let place = places.entry((group, parent)).or_insert(0);
            let first = *place;
            *place += 1;
            if let Some(tag) = self.tags.iter_mut().find(|t| t.id == id) {
                tag.first = first;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (TagStore, i64, i64) {
        let store = TagStore::new();
        let kick = store.find("Function|Kick")[0];
        let loop_ = store.find("Function|Loop")[0];
        (store, kick, loop_)
    }

    // Spec: browser-tags R50. Every tag the formats' tables and the sound
    // classifier can give is in the Function group's list.
    #[test]
    fn what_the_tables_and_the_classifier_give_is_in_the_function_list() {
        let store = TagStore::new();
        let given = bl_plugin_host::categories::all_function_tags()
            .chain(bl_analysis::autotag::classes().iter().copied())
            .chain(["Loop", "One-Shot"]);
        for name in given {
            assert!(FUNCTION_TAGS.contains(&name), "{name}");
            assert_eq!(store.find(&format!("Function|{name}")).len(), 1, "{name}");
        }
        let group = store
            .group(store.group_id(FUNCTION_GROUP).unwrap())
            .unwrap();
        assert!(
            group.builtin,
            "Beatlayer's group can't be renamed or deleted"
        );
    }

    // Spec: browser-tags R51, R52. A removed automatic tag is remembered and
    // not given again, whichever source gives it; the other tags are kept.
    #[test]
    fn a_removed_automatic_tag_is_not_given_again() {
        let (mut store, kick, loop_) = store();
        let ops = store.set_auto_tags("/a.wav", AutoSource::Sound, [kick, loop_]);
        assert_eq!(ops.len(), 1);
        let ops = store.remove_auto_tag("/a.wav", kick);
        assert_eq!(ops.len(), 2, "the tags left and the one removed are kept");
        assert_eq!(store.removed_auto_tags("/a.wav"), [kick]);
        // A new analysis finds both again; only the one still wanted stays.
        store.set_auto_tags("/a.wav", AutoSource::Sound, [kick, loop_]);
        let kept: Vec<i64> = store
            .auto_tags_ref("/a.wav")
            .unwrap()
            .keys()
            .copied()
            .collect();
        assert_eq!(kept, [loop_]);
        // The same answer again changes nothing to write.
        assert!(
            store
                .set_auto_tags("/a.wav", AutoSource::Sound, [kick, loop_])
                .is_empty()
        );
        // A tag the item doesn't have can't be removed.
        assert!(store.remove_auto_tag("/b.wav", kick).is_empty());
        // A person's own tags are another set, which none of this touches.
        assert!(store.item_tags("/a.wav").is_empty());
    }

    // Spec: browser-tags R51. Each source's tags are replaced by that source
    // alone.
    #[test]
    fn a_source_replaces_only_its_own_tags() {
        let (mut store, kick, loop_) = store();
        store.set_auto_tags("/a", AutoSource::Plugin, [kick]);
        store.set_auto_tags("/a", AutoSource::Sound, [loop_]);
        store.set_auto_tags("/a", AutoSource::Sound, []);
        let now = store.auto_tags_ref("/a").unwrap();
        assert_eq!(now.get(&kick), Some(&AutoSource::Plugin));
        assert!(!now.contains_key(&loop_));
        store.set_auto_tags("/a", AutoSource::Plugin, []);
        assert!(store.auto_tags_ref("/a").is_none());
    }

    // Spec: browser-tags R5, R51. What a person removed passes to the file's
    // new path, and takes the tag off what that path was given meanwhile.
    #[test]
    fn what_was_removed_moves_with_the_file() {
        let (mut store, kick, _) = store();
        store.set_auto_tags("/old", AutoSource::Sound, [kick]);
        store.remove_auto_tag("/old", kick);
        store.set_auto_tags("/new", AutoSource::Sound, [kick]);
        assert!(store.auto_tags_ref("/new").is_some());
        let ops = store.move_removed_auto_tags("/old", "/new");
        assert!(!ops.is_empty());
        assert!(store.removed_auto_tags("/old").is_empty());
        assert_eq!(store.removed_auto_tags("/new"), [kick]);
        assert!(store.auto_tags_ref("/new").is_none());
    }

    // Spec: browser-tags R49, R56. A database from before the Function group
    // gets it, with IDs that clash with none of the person's.
    #[test]
    fn a_database_without_the_function_group_gains_it_beside_a_persons_own() {
        let mut first = TagStore::new();
        // Make the store as an older build wrote it: no Function group, and a
        // person's group and tag made with the IDs it would have used.
        first.drop_function();
        first.recount();
        let (group, _) = first.add_group("Mine").unwrap();
        let (tag, _) = first.add_tag("Hard", group, None).unwrap();
        let groups: Vec<TagGroup> = first.groups.clone();
        let tags: Vec<Tag> = first.tags.clone();
        assert!(first.group_id(FUNCTION_GROUP).is_none());

        let mut second = TagStore::new();
        let ops = second.load(groups, tags, Vec::new(), Vec::new(), Vec::new());
        assert!(
            second.group_id(FUNCTION_GROUP).is_some(),
            "the Function group"
        );
        assert_eq!(second.group(group).unwrap().name, "Mine");
        assert_eq!(second.tag(tag).unwrap().name, "Hard");
        assert!(
            ops.iter()
                .any(|op| matches!(op, TagOp::AddGroup { name, .. } if name == FUNCTION_GROUP)),
            "the database is told of it"
        );
        let ids: BTreeSet<i64> = second
            .groups
            .iter()
            .map(|g| g.id)
            .chain(second.tags.iter().map(|t| t.id))
            .collect();
        assert_eq!(
            ids.len(),
            second.groups.len() + second.tags.len(),
            "no ID twice"
        );
    }
}
