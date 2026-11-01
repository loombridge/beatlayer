//! The Filter View (spec: browser-tags R18 to R22): which tag groups each
//! label shows, the tags selected in them, and which items they leave in
//! the list.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use bl_shell_spec::browser::{
    FilterGroup, FilterGroupChoice, FilterTag, FilterViewState, Item, ItemKind, Label,
};
use serde::{Deserialize, Serialize};

use super::{BrowserError, BrowserView, label_devices, listing};

/// What the person changed about the groups a label shows, on top of the
/// default ones (spec: browser-tags R19, R21).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupChoice {
    /// Groups the label shows although it doesn't by default.
    pub show: BTreeSet<String>,
    /// Groups the label hides although it shows them by default.
    pub hide: BTreeSet<String>,
}

/// The Filter View, kept between launches (spec: browser-tags R18, R21).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterPrefs {
    /// Whether the Filter View shows. On in a new install.
    pub shown: bool,
    /// What the person changed for each label, by [`Label::key`].
    pub groups: BTreeMap<String, GroupChoice>,
    /// Whether the Filter View Menu's Include Auto Tags is on: automatic
    /// tags show in the Filter View, filter and answer `#` search (spec:
    /// browser-tags R55). On in a new install.
    pub include_auto_tags: bool,
}

impl Default for FilterPrefs {
    fn default() -> Self {
        Self {
            shown: true,
            groups: BTreeMap::new(),
            include_auto_tags: true,
        }
    }
}

/// Whether a label shows a group in a new install (spec: browser-tags R19,
/// Beatlayer's own choice).
fn default_shown(label: &Label, group: &str, builtin: bool) -> bool {
    if !builtin {
        return true;
    }
    let is = |names: &[&str]| names.iter().any(|name| name.eq_ignore_ascii_case(group));
    match label {
        Label::All => true,
        Label::PlugIns => is(&["Devices", "Format", "Creator"]),
        Label::Instruments
        | Label::AudioEffects
        | Label::MidiEffects
        | Label::Drums
        | Label::Modulators => is(&["Devices", "Content"]),
        _ => is(&["Content"]),
    }
}

impl FilterPrefs {
    /// Whether `label` shows the group.
    #[must_use]
    pub fn group_shown(&self, label: &Label, group: &str, builtin: bool) -> bool {
        match self.groups.get(&label.key()) {
            Some(choice) if choice.hide.contains(group) => false,
            Some(choice) if choice.show.contains(group) => true,
            _ => default_shown(label, group, builtin),
        }
    }

    /// Shows or hides a group under a label.
    pub fn set_group(&mut self, label: &Label, group: &str, builtin: bool, shown: bool) {
        let default = default_shown(label, group, builtin);
        let choice = self.groups.entry(label.key()).or_default();
        choice.show.remove(group);
        choice.hide.remove(group);
        if shown && !default {
            choice.show.insert(group.to_owned());
        } else if !shown && default {
            choice.hide.insert(group.to_owned());
        }
        if choice.show.is_empty() && choice.hide.is_empty() {
            self.groups.remove(&label.key());
        }
    }

    /// Makes `label` show every group of these.
    pub fn show_all(&mut self, label: &Label, groups: &[(String, bool)]) {
        for (name, builtin) in groups {
            self.set_group(label, name, *builtin, true);
        }
    }

    /// Puts a label's groups back to the default ones.
    pub fn reset(&mut self, label: &Label) {
        self.groups.remove(&label.key());
    }
}

/// The tags selected in the groups a label shows, ready to test items
/// against (spec: browser-tags R20): each group with a selection needs one
/// of its selected tags, a tag counting for the subtags under it too.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    groups: Vec<Vec<Vec<i64>>>,
}

impl Selection {
    /// Whether nothing is selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Whether an item that carries these tags is left in the list.
    #[must_use]
    pub fn matches(&self, carried: &[i64]) -> bool {
        self.groups.iter().all(|group| {
            group
                .iter()
                .any(|ids| ids.iter().any(|id| carried.contains(id)))
        })
    }
}

/// The IDs of the tags Beatlayer gives files by what they are (spec:
/// browser-tags R7), looked up once for many files.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuiltinIds {
    sample: Option<i64>,
    audio_clip: Option<i64>,
    midi_clip: Option<i64>,
    preset: Option<i64>,
    set: Option<i64>,
    backup_set: Option<i64>,
}

impl BrowserView {
    /// The label whose items the list holds: a saved label lists what its
    /// words and tags find under the label it was made under.
    pub(super) fn base_label(&self) -> Label {
        match &self.label {
            Label::Saved(id) => self
                .saved
                .iter()
                .find(|saved| saved.id == *id)
                .map_or(Label::All, |saved| saved.base.clone()),
            other => other.clone(),
        }
    }

    /// Whether the Filter View shows.
    #[must_use]
    pub fn filter_view_shown(&self) -> bool {
        self.filter_prefs.shown
    }

    /// The selected filter tags' full names.
    #[must_use]
    pub fn filter_tags(&self) -> Vec<String> {
        self.filter
            .iter()
            .map(|&id| self.tags.full_name(id))
            .filter(|name| !name.is_empty())
            .collect()
    }

    /// How many filter tags are selected, which the Results bar says.
    #[must_use]
    pub fn filter_count(&self) -> usize {
        self.filter.len()
    }

    /// Whether the Results bar shows: a search or a filter is active (spec:
    /// browser-tags R22).
    #[must_use]
    pub fn results_bar(&self) -> bool {
        !self.search.trim().is_empty() || !self.filter.is_empty()
    }

    /// The tags selected in the groups the chosen label shows.
    pub(super) fn applied_selection(&self) -> Selection {
        let mut by_group: BTreeMap<i64, Vec<Vec<i64>>> = BTreeMap::new();
        for &id in &self.filter {
            let Some(tag) = self.tags.tag(id) else {
                continue;
            };
            let Some(group) = self.tags.group(tag.group) else {
                continue;
            };
            if !self
                .filter_prefs
                .group_shown(&self.label, &group.name, group.builtin)
            {
                continue;
            }
            let mut ids = vec![id];
            ids.extend(self.tags.subtags(id).iter().map(|sub| sub.id));
            by_group.entry(tag.group).or_default().push(ids);
        }
        Selection {
            groups: by_group.into_values().collect(),
        }
    }

    /// The IDs of the tags Beatlayer gives files.
    pub(super) fn builtin_ids(&self) -> BuiltinIds {
        let find = |name: &str| self.tags.find(name).first().copied();
        BuiltinIds {
            sample: find("Content|Sample"),
            audio_clip: find("Content|Audio Clip"),
            midi_clip: find("Content|MIDI Clip"),
            preset: find("Content|Preset"),
            set: find("Content|Set"),
            backup_set: find("Content|Backup Set"),
        }
    }

    /// The tags a file or folder carries, in its own right, through the
    /// folders it lies in, and as Beatlayer gives them (spec: browser-tags
    /// R7, R9).
    pub(super) fn file_ids(&self, path: &Path, kind: ItemKind, builtin: &BuiltinIds) -> Vec<i64> {
        let mut ids: Vec<i64> = Vec::new();
        let mut at = Some(path);
        let mut depth = 0;
        while let Some(p) = at {
            if let Some(tags) = self.tags.item_tags_ref(&p.to_string_lossy()) {
                ids.extend(tags.iter().copied());
            }
            at = p.parent();
            depth += 1;
            if depth > 32 {
                break;
            }
        }
        let given = match kind {
            ItemKind::Audio => builtin.sample,
            ItemKind::Clip => match listing::clip_kind(path) {
                Some(bl_shell_spec::browser::ClipFileKind::Audio) => builtin.audio_clip,
                Some(bl_shell_spec::browser::ClipFileKind::Midi) => builtin.midi_clip,
                None => None,
            },
            ItemKind::Preset => builtin.preset,
            ItemKind::Set if self.is_backup_set(path) => builtin.backup_set,
            ItemKind::Set => builtin.set,
            _ => None,
        };
        ids.extend(given);
        // Spec: browser-tags R55. A sample's automatic tags, while Include
        // Auto Tags is on.
        if self.filter_prefs.include_auto_tags
            && let Some(auto) = self.tags.auto_tags_ref(&path.to_string_lossy())
        {
            ids.extend(auto.keys().copied());
        }
        ids
    }

    /// Whether an item the list holds passes the selected tags.
    pub(super) fn item_passes(
        &self,
        selection: &Selection,
        builtin: &BuiltinIds,
        item: &Item,
    ) -> bool {
        if selection.is_empty() {
            return true;
        }
        let carried: Vec<i64> = match item.kind {
            ItemKind::Plugin | ItemKind::Device => self
                .searchable_tags(&self.tag_key(&item.path))
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
            kind => self.file_ids(Path::new(&item.path), kind, builtin),
        };
        selection.matches(&carried)
    }

    /// The tags some item under `label` carries, a subtag making its tag
    /// count (spec: browser-tags R19). Worked out once for as long as the
    /// index, the plug-ins and the tags stay as they are.
    fn available_tags(&self) -> BTreeSet<i64> {
        let base = self.base_label();
        let key = (self.generation, base.key());
        if let Ok(cache) = self.available.lock()
            && let Some((cached, tags)) = cache.as_ref()
            && *cached == key
        {
            return tags.clone();
        }
        let tags = self.compute_available(&base);
        if let Ok(mut cache) = self.available.lock() {
            *cache = Some((key, tags.clone()));
        }
        tags
    }

    fn compute_available(&self, base: &Label) -> BTreeSet<i64> {
        let mut found: BTreeSet<i64> = BTreeSet::new();
        let builtin = self.builtin_ids();
        let devices = label_devices(base);
        if !devices.is_empty() {
            for device in devices {
                found.extend(
                    self.searchable_tags(&device.id)
                        .into_iter()
                        .map(|(id, _)| id),
                );
            }
        } else if *base == Label::PlugIns {
            for plugin in &self.plugins {
                found.extend(
                    self.searchable_tags(&plugin.key())
                        .into_iter()
                        .map(|(id, _)| id),
                );
            }
        } else if let Some((scope, only)) = self.scope(base) {
            let snapshot = self.index.snapshot();
            let mut folders: HashMap<u32, Vec<i64>> = HashMap::new();
            for hit in snapshot.search("", scope, only) {
                let entry = snapshot.entry(hit);
                // The tags of the folders the item lies in, once for each.
                let above = folders.entry(snapshot.folder_of(hit)).or_insert_with(|| {
                    let mut ids = Vec::new();
                    for dir in snapshot.folder_path(snapshot.folder_of(hit)).ancestors() {
                        if let Some(tags) = self.tags.item_tags_ref(&dir.to_string_lossy()) {
                            ids.extend(tags.iter().copied());
                        }
                    }
                    ids
                });
                found.extend(above.iter().copied());
                found.extend(self.file_ids(&snapshot.path(hit), entry.kind, &builtin));
            }
        } else if *base == Label::Templates
            && let Some(folder) = self.templates_folder()
            && let Ok(entries) = listing::list(&folder)
        {
            for entry in entries {
                found.extend(self.file_ids(&entry.path, entry.kind, &builtin));
            }
        }
        // An item that carries a subtag also carries its tag (spec:
        // browser-tags R6).
        let parents: Vec<i64> = found
            .iter()
            .filter_map(|&id| self.tags.tag(id).and_then(|tag| tag.parent))
            .collect();
        found.extend(parents);
        found
    }

    /// The Filter View as `browser.state` reports it and the UI draws it.
    #[must_use]
    pub fn filter_view(&self) -> FilterViewState {
        let groups = self.tags.groups();
        let menu: Vec<FilterGroupChoice> = groups
            .iter()
            .map(|group| FilterGroupChoice {
                name: group.name.clone(),
                shown: self
                    .filter_prefs
                    .group_shown(&self.label, &group.name, group.builtin),
            })
            .collect();
        let available = self.available_tags();
        let mut shown_groups = Vec::new();
        for (group, choice) in groups.iter().zip(&menu) {
            if !choice.shown {
                continue;
            }
            let mut tags = Vec::new();
            for tag in self.tags.tags_in(group.id) {
                let subs: Vec<_> = self
                    .tags
                    .subtags(tag.id)
                    .into_iter()
                    .filter(|sub| available.contains(&sub.id))
                    .collect();
                if !available.contains(&tag.id) && subs.is_empty() {
                    continue;
                }
                tags.push(FilterTag {
                    name: tag.name.clone(),
                    full_name: self.tags.full_name(tag.id),
                    selected: self.filter.contains(&tag.id),
                });
                for sub in subs {
                    tags.push(FilterTag {
                        name: format!("{}|{}", tag.name, sub.name),
                        full_name: self.tags.full_name(sub.id),
                        selected: self.filter.contains(&sub.id),
                    });
                }
            }
            if !tags.is_empty() {
                shown_groups.push(FilterGroup {
                    name: group.name.clone(),
                    tags,
                });
            }
        }
        FilterViewState {
            shown: self.filter_prefs.shown,
            groups: shown_groups,
            menu,
            selected: self.filter_tags(),
            include_auto_tags: self.filter_prefs.include_auto_tags,
        }
    }

    /// Shows or hides the Filter View; whether the preferences changed.
    pub(super) fn set_filter_view(&mut self, show: Option<bool>) {
        self.filter_prefs.shown = show.unwrap_or(!self.filter_prefs.shown);
    }

    /// Shows, hides, shows all or resets the groups a label shows (spec:
    /// browser-tags R21).
    pub(super) fn set_filter_groups(
        &mut self,
        args: bl_shell_spec::browser::SetFilterGroupsArgs,
    ) -> Result<(), BrowserError> {
        let label = args.label.unwrap_or_else(|| self.label.clone());
        if let Label::Folder(_) = &label {
            self.check_label(&label)?;
        }
        let modes = usize::from(args.group.is_some())
            + usize::from(args.show_all)
            + usize::from(args.reset);
        if modes != 1 {
            return Err(BrowserError::InvalidArgs(
                "give one of `group`, `show_all` and `reset`".to_owned(),
            ));
        }
        if args.reset {
            self.filter_prefs.reset(&label);
        } else if args.show_all {
            let groups: Vec<(String, bool)> = self
                .tags
                .groups()
                .iter()
                .map(|group| (group.name.clone(), group.builtin))
                .collect();
            self.filter_prefs.show_all(&label, &groups);
        } else if let Some(group) = args.group {
            let found = self
                .tags
                .groups()
                .into_iter()
                .find(|g| g.name.eq_ignore_ascii_case(&group))
                .map(|g| (g.name.clone(), g.builtin))
                .ok_or_else(|| {
                    BrowserError::InvalidArgs(format!("there's no group called {group}"))
                })?;
            self.filter_prefs
                .set_group(&label, &found.0, found.1, args.shown.unwrap_or(true));
        }
        Ok(())
    }

    /// Selects filter tags (spec: browser-tags R20).
    pub(super) fn select_filter_tags(
        &mut self,
        tags: &[String],
        add: bool,
    ) -> Result<(), BrowserError> {
        if tags.is_empty() {
            return Err(BrowserError::InvalidArgs(
                "name at least one tag".to_owned(),
            ));
        }
        let mut wanted: Vec<i64> = Vec::new();
        for name in tags {
            let id = self.tag_id(name)?;
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        if add {
            for id in wanted {
                if !self.filter.remove(&id) {
                    self.filter.insert(id);
                }
            }
            return Ok(());
        }
        let group_of = |view: &Self, id: i64| view.tags.tag(id).map(|tag| tag.group);
        let groups: BTreeSet<i64> = wanted.iter().filter_map(|&id| group_of(self, id)).collect();
        for group in groups {
            let new: BTreeSet<i64> = wanted
                .iter()
                .copied()
                .filter(|&id| group_of(self, id) == Some(group))
                .collect();
            let old: BTreeSet<i64> = self
                .filter
                .iter()
                .copied()
                .filter(|&id| group_of(self, id) == Some(group))
                .collect();
            for id in &old {
                self.filter.remove(id);
            }
            // A tag that is the only one selected, clicked again, clears.
            if !(new == old && new.len() == 1) {
                self.filter.extend(new);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: browser-tags R19.
    #[test]
    fn labels_show_the_groups_the_spec_gives_them() {
        let prefs = FilterPrefs::default();
        let shown = |label: &Label, group: &str| prefs.group_shown(label, group, true);
        for group in ["Content", "Format", "Creator", "Devices", "Function"] {
            assert!(shown(&Label::All, group), "{group} under All");
        }
        assert!(shown(&Label::PlugIns, "Devices"));
        assert!(!shown(&Label::PlugIns, "Function"), "R19 doesn't list it");
        assert!(shown(&Label::PlugIns, "Format"));
        assert!(shown(&Label::PlugIns, "Creator"));
        assert!(!shown(&Label::PlugIns, "Content"));
        assert!(shown(&Label::AudioEffects, "Devices"));
        assert!(shown(&Label::AudioEffects, "Content"));
        assert!(!shown(&Label::AudioEffects, "Creator"));
        assert!(shown(&Label::Samples, "Content"));
        assert!(!shown(&Label::Samples, "Devices"));
        assert!(shown(&Label::UserLibrary, "Content"));
        assert!(shown(&Label::Saved(3), "Content"));
        assert!(
            prefs.group_shown(&Label::Samples, "Mine", false),
            "own groups show"
        );
    }

    // Spec: browser-tags R21.
    #[test]
    fn showing_hiding_and_resetting_a_labels_groups() {
        let mut prefs = FilterPrefs::default();
        let label = Label::Samples;
        prefs.set_group(&label, "Format", true, true);
        assert!(prefs.group_shown(&label, "Format", true));
        prefs.set_group(&label, "Content", true, false);
        assert!(!prefs.group_shown(&label, "Content", true));
        prefs.reset(&label);
        assert!(!prefs.group_shown(&label, "Format", true));
        assert!(prefs.group_shown(&label, "Content", true));
        prefs.show_all(&label, &[("Creator".to_owned(), true)]);
        assert!(prefs.group_shown(&label, "Creator", true));
        // Back to the default: nothing is kept for the label.
        prefs.set_group(&label, "Creator", true, false);
        assert!(prefs.groups.is_empty());
    }

    // Spec: browser-tags R20.
    #[test]
    fn tags_in_one_group_widen_and_groups_narrow() {
        let selection = Selection {
            groups: vec![vec![vec![1], vec![2, 20]], vec![vec![3]]],
        };
        assert!(selection.matches(&[1, 3]));
        assert!(selection.matches(&[20, 3]), "a subtag counts for its tag");
        assert!(!selection.matches(&[1]), "the other group needs one too");
        assert!(!selection.matches(&[3]));
        assert!(Selection::default().matches(&[]));
    }
}
