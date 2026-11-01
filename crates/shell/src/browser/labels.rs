//! The sidebar's labels as the person arranges them (spec: browser-tags R24
//! to R26): saved labels, and the order, visibility, names and icons of the
//! Library labels.

use std::collections::BTreeMap;

use bl_shell_spec::browser::{
    LABEL_ICONS, Label, MoveLabelArgs, RemoveLabelArgs, RenameLabelArgs, SavedLabelInfo,
    SetLabelIconArgs, SetLabelShownArgs,
};
use serde::{Deserialize, Serialize};

use super::db::LabelOp;
use super::{BrowserError, BrowserView};

/// A saved label: what the browser showed when Add Label was pressed (spec:
/// browser-tags R24).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedLabel {
    /// Its ID, which the database keeps.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// The label it was made under.
    pub base: Label,
    /// The search words it keeps.
    pub search: String,
    /// The icon the person gave it.
    pub icon: Option<String>,
    /// The tags it keeps, by their IDs.
    pub tags: Vec<i64>,
    /// The file whose neighbours it lists, reference first (spec:
    /// sound-similarity R15).
    pub reference: Option<String>,
}

impl SavedLabel {
    /// The label that stands for it in the sidebar.
    #[must_use]
    pub const fn label(&self) -> Label {
        Label::Saved(self.id)
    }
}

/// What the person has done to the labels, kept between launches (spec:
/// browser-tags R25, R26).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabelPrefs {
    /// The Library and saved labels in the order the person left them, by
    /// [`Label::key`]; a label it doesn't name follows in its first place.
    pub order: Vec<String>,
    /// The labels the sidebar leaves out, by [`Label::key`].
    pub hidden: Vec<String>,
    /// The names the person gave, by [`Label::key`].
    pub names: BTreeMap<String, String>,
    /// The icons the person gave, by [`Label::key`].
    pub icons: BTreeMap<String, String>,
}

impl LabelPrefs {
    /// The Library section's labels in the order the sidebar lists them:
    /// All first, then the labels in the saved order, the ones it doesn't
    /// name after them in their first order (Library labels, then saved
    /// labels).
    #[must_use]
    pub fn ordered(&self, saved: &[SavedLabel]) -> Vec<Label> {
        let mut first: Vec<Label> = Label::LIBRARY.to_vec();
        first.extend(saved.iter().map(SavedLabel::label));
        let mut ordered: Vec<Label> = vec![Label::All];
        for key in &self.order {
            if let Some(label) = first.iter().find(|l| l.key() == *key)
                && !ordered.contains(label)
            {
                ordered.push(label.clone());
            }
        }
        for label in first {
            if !ordered.contains(&label) {
                ordered.push(label);
            }
        }
        ordered
    }

    /// Whether the sidebar shows the label.
    #[must_use]
    pub fn shown(&self, label: &Label) -> bool {
        *label == Label::All || !self.hidden.contains(&label.key())
    }

    /// Shows or hides a label.
    pub fn set_shown(&mut self, label: &Label, shown: bool) {
        if *label == Label::All {
            return;
        }
        let key = label.key();
        self.hidden.retain(|k| *k != key);
        if !shown {
            self.hidden.push(key);
        }
    }

    /// Puts `label` at `position` among the Library section's labels,
    /// `ordered` being their present order. All keeps the first place.
    pub fn move_label(&mut self, ordered: &[Label], label: &Label, position: usize) {
        let mut labels: Vec<Label> = ordered.to_vec();
        labels.retain(|l| l != label);
        labels.insert(position.clamp(1, labels.len()), label.clone());
        self.order = labels.iter().map(Label::key).collect();
    }

    /// Forgets the order the person left.
    pub fn reset_order(&mut self) {
        self.order.clear();
    }

    /// The name the person gave the label, if they did.
    #[must_use]
    pub fn name(&self, label: &Label) -> Option<&str> {
        self.names.get(&label.key()).map(String::as_str)
    }

    /// The icon the person gave the label, if they did.
    #[must_use]
    pub fn icon(&self, label: &Label) -> Option<&str> {
        self.icons.get(&label.key()).map(String::as_str)
    }

    /// Forgets what is kept for a label that is gone.
    pub fn forget(&mut self, label: &Label) {
        let key = label.key();
        self.order.retain(|k| *k != key);
        self.hidden.retain(|k| *k != key);
        self.names.remove(&key);
        self.icons.remove(&key);
    }
}

/// A name for a new saved label: its search words and its tags' own names,
/// after "Similar to" and the reference's name for a list of neighbours.
#[must_use]
pub fn default_name(search: &str, tags: &[String], reference: Option<&str>) -> String {
    let mut parts: Vec<&str> = search.split_whitespace().collect();
    parts.extend(tags.iter().map(String::as_str));
    let name = if parts.is_empty() {
        "New Label".to_owned()
    } else {
        parts.join(" ")
    };
    match reference.and_then(|path| std::path::Path::new(path).file_stem()) {
        Some(stem) if parts.is_empty() => format!("Similar to {}", stem.to_string_lossy()),
        Some(stem) => format!("Similar to {} {name}", stem.to_string_lossy()),
        None => name,
    }
}

impl BrowserView {
    /// The saved labels, as `browser.state` reports them.
    #[must_use]
    pub fn saved_labels(&self) -> Vec<SavedLabelInfo> {
        self.saved
            .iter()
            .map(|saved| SavedLabelInfo {
                label: saved.label(),
                name: saved.name.clone(),
                icon: saved.icon.clone(),
                base: saved.base.clone(),
                search: saved.search.clone(),
                tags: saved
                    .tags
                    .iter()
                    .map(|&id| self.tags.full_name(id))
                    .filter(|name| !name.is_empty())
                    .collect(),
                reference: saved.reference.clone(),
            })
            .collect()
    }

    /// The label whose name is open for editing.
    #[must_use]
    pub fn renaming_label(&self) -> Option<&Label> {
        self.renaming_label.as_ref()
    }

    /// The saved label with this ID.
    fn saved_label(&self, id: i64) -> Result<&SavedLabel, BrowserError> {
        self.saved
            .iter()
            .find(|saved| saved.id == id)
            .ok_or_else(|| BrowserError::InvalidArgs(format!("no saved label has the ID {id}")))
    }

    /// Writes a saved label to the database.
    fn save_label(&self, id: i64) {
        if let (Some(db), Some(saved)) = (&self.db, self.saved.iter().find(|s| s.id == id)) {
            db.save_labels(vec![LabelOp::Save(saved.clone())]);
        }
    }

    /// Whether `label` is one the sidebar lists in its Library section and
    /// exists.
    fn check_library_label(&self, label: &Label) -> Result<(), BrowserError> {
        if !label.is_library() {
            return Err(BrowserError::InvalidArgs(format!(
                "{} isn't a Library label or a saved label",
                self.label_name(label)
            )));
        }
        if let Label::Saved(id) = label {
            self.saved_label(*id)?;
        }
        Ok(())
    }

    /// Whether `label` is a label the sidebar lets the person rename and
    /// hide: a Library label, a saved label or a collection, that exists.
    fn check_shown_label(&self, label: &Label) -> Result<(), BrowserError> {
        if label.is_collection() {
            return self.check_label(label);
        }
        self.check_library_label(label)
    }

    /// Keeps what the browser shows as a saved label and chooses it, its
    /// name open for editing (spec: browser-tags R24).
    pub(super) fn add_label(&mut self) -> Result<(), BrowserError> {
        let reference = self
            .reference()
            .map(|path| path.to_string_lossy().into_owned());
        if self.search.trim().is_empty() && self.filter.is_empty() && reference.is_none() {
            return Err(BrowserError::Unavailable(
                "there is nothing to keep: type search words or select filter tags first"
                    .to_owned(),
            ));
        }
        let id = self.saved.iter().map(|saved| saved.id).max().unwrap_or(0) + 1;
        let tags: Vec<i64> = self.filter.iter().copied().collect();
        let names: Vec<String> = tags
            .iter()
            .filter_map(|&tag| self.tags.tag(tag).map(|tag| tag.name.clone()))
            .collect();
        let saved = SavedLabel {
            id,
            name: default_name(&self.search, &names, reference.as_deref()),
            base: self.base_label(),
            search: self.search.trim().to_owned(),
            icon: None,
            tags,
            reference,
        };
        self.saved.push(saved);
        self.save_label(id);
        self.label = Label::Saved(id);
        self.renaming_label = Some(Label::Saved(id));
        self.generation += 1;
        self.rebuild();
        Ok(())
    }

    /// Chooses a saved label: its words and tags are the search and the
    /// selected tags now (spec: browser-tags R24).
    pub(super) fn choose_saved(&mut self, id: i64) -> Result<(), BrowserError> {
        let saved = self.saved_label(id)?.clone();
        self.label = saved.label();
        self.similar = None;
        self.search = saved.search;
        self.filter = saved
            .tags
            .into_iter()
            .filter(|&tag| self.tags.tag(tag).is_some())
            .collect();
        Ok(())
    }

    /// Renames a label, or opens its name for editing; whether the
    /// preferences changed.
    pub(super) fn rename_label(&mut self, args: RenameLabelArgs) -> Result<bool, BrowserError> {
        let RenameLabelArgs { label, name } = args;
        let label = label.unwrap_or_else(|| self.label.clone());
        if !label.is_library() && !label.is_collection() {
            return Err(BrowserError::InvalidArgs(format!(
                "{} takes its folder's name and isn't renamed",
                self.label_name(&label)
            )));
        }
        self.check_shown_label(&label)?;
        let Some(name) = name else {
            self.renaming_label = if self.renaming_label.as_ref() == Some(&label) {
                None
            } else {
                Some(label)
            };
            return Ok(false);
        };
        if self.renaming_label.as_ref() == Some(&label) {
            self.renaming_label = None;
        }
        let name = name.trim().to_owned();
        if let Label::Saved(id) = label {
            if name.is_empty() {
                return Err(BrowserError::InvalidArgs(
                    "a saved label's name can't be empty".to_owned(),
                ));
            }
            if let Some(saved) = self.saved.iter_mut().find(|saved| saved.id == id) {
                saved.name = name;
            }
            self.save_label(id);
            return Ok(false);
        }
        if name.is_empty() || name == label.name() {
            self.label_prefs.names.remove(&label.key());
        } else {
            self.label_prefs.names.insert(label.key(), name);
        }
        Ok(true)
    }

    /// Gives a label an icon, or takes it away; whether the preferences
    /// changed.
    pub(super) fn set_label_icon(&mut self, args: SetLabelIconArgs) -> Result<bool, BrowserError> {
        let SetLabelIconArgs { label, icon } = args;
        if matches!(
            label,
            Label::All | Label::UserLibrary | Label::CurrentProject
        ) {
            return Err(BrowserError::InvalidArgs(format!(
                "{} keeps its icon",
                label.name()
            )));
        }
        if label.is_collection() {
            return Err(BrowserError::InvalidArgs(format!(
                "{} has its colour, not an icon",
                self.label_name(&label)
            )));
        }
        if let Label::Folder(_) = &label {
            self.check_label(&label)?;
        } else {
            self.check_library_label(&label)?;
        }
        if let Some(icon) = &icon
            && !LABEL_ICONS.contains(&icon.as_str())
        {
            return Err(BrowserError::InvalidArgs(format!(
                "{icon} isn't one of the icons: {}",
                LABEL_ICONS.join(", ")
            )));
        }
        if let Label::Saved(id) = label {
            if let Some(saved) = self.saved.iter_mut().find(|saved| saved.id == id) {
                saved.icon = icon;
            }
            self.save_label(id);
            return Ok(false);
        }
        match icon {
            Some(icon) => {
                self.label_prefs.icons.insert(label.key(), icon);
            }
            None => {
                self.label_prefs.icons.remove(&label.key());
            }
        }
        Ok(true)
    }

    /// Shows or hides a Library or saved label in the sidebar.
    pub(super) fn set_label_shown(&mut self, args: SetLabelShownArgs) -> Result<(), BrowserError> {
        let SetLabelShownArgs { label, shown } = args;
        self.check_shown_label(&label)?;
        if label == Label::All && !shown {
            return Err(BrowserError::InvalidArgs("All can't be hidden".to_owned()));
        }
        self.label_prefs.set_shown(&label, shown);
        if !shown && self.label == label {
            self.label = Label::All;
            self.rebuild();
        }
        Ok(())
    }

    /// Moves a label within the Library section.
    pub(super) fn move_label(&mut self, args: MoveLabelArgs) -> Result<(), BrowserError> {
        let MoveLabelArgs { label, position } = args;
        if label.is_collection() {
            return Err(BrowserError::InvalidArgs(format!(
                "{} keeps its place: the collections are always in the same order",
                self.label_name(&label)
            )));
        }
        self.check_library_label(&label)?;
        if label == Label::All {
            return Err(BrowserError::InvalidArgs("All stays first".to_owned()));
        }
        let ordered = self.label_prefs.ordered(&self.saved);
        self.label_prefs.move_label(&ordered, &label, position);
        Ok(())
    }

    /// Puts the Library labels back in their first order.
    pub(super) fn reset_label_order(&mut self) {
        self.label_prefs.reset_order();
    }

    /// Deletes a saved label and nothing else.
    pub(super) fn remove_label(&mut self, args: RemoveLabelArgs) -> Result<(), BrowserError> {
        let Label::Saved(id) = args.label else {
            return Err(BrowserError::InvalidArgs(format!(
                "{} isn't a saved label",
                self.label_name(&args.label)
            )));
        };
        self.saved_label(id)?;
        self.saved.retain(|saved| saved.id != id);
        let label = Label::Saved(id);
        self.label_prefs.forget(&label);
        self.filter_prefs.reset(&label);
        if let Some(db) = &self.db {
            db.save_labels(vec![LabelOp::Delete { id }]);
        }
        if self.renaming_label.as_ref() == Some(&label) {
            self.renaming_label = None;
        }
        if self.label == label {
            self.label = Label::All;
            self.rebuild();
        }
        Ok(())
    }

    /// The saved labels that use any of these tags, by name.
    pub(super) fn labels_using(&self, tags: &[i64]) -> Vec<String> {
        self.saved
            .iter()
            .filter(|saved| saved.tags.iter().any(|tag| tags.contains(tag)))
            .map(|saved| saved.name.clone())
            .collect()
    }

    /// Takes tags that are gone out of the saved labels and the selected
    /// tags (the database already has).
    pub(super) fn forget_gone_tags(&mut self) {
        let tags = &self.tags;
        for saved in &mut self.saved {
            saved.tags.retain(|&tag| tags.tag(tag).is_some());
        }
        self.filter.retain(|&tag| tags.tag(tag).is_some());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(id: i64) -> SavedLabel {
        SavedLabel {
            id,
            name: format!("Saved {id}"),
            base: Label::All,
            search: String::new(),
            icon: None,
            tags: Vec::new(),
            reference: None,
        }
    }

    // Spec: browser-tags R26.
    #[test]
    fn all_stays_first_and_saved_labels_follow_the_library() {
        let mut prefs = LabelPrefs::default();
        let saved = [saved(1), saved(2)];
        let order = prefs.ordered(&saved);
        assert_eq!(order[0], Label::All);
        assert_eq!(order[13], Label::Saved(1));
        assert_eq!(order[14], Label::Saved(2));
        prefs.move_label(&order, &Label::Samples, 0);
        let order = prefs.ordered(&saved);
        assert_eq!(order[0], Label::All, "All stays first");
        assert_eq!(order[1], Label::Samples);
        prefs.move_label(&order, &Label::Saved(2), 2);
        assert_eq!(prefs.ordered(&saved)[2], Label::Saved(2));
        prefs.reset_order();
        assert_eq!(prefs.ordered(&saved)[1], Label::Sounds);
    }

    #[test]
    fn all_is_never_hidden() {
        let mut prefs = LabelPrefs::default();
        prefs.set_shown(&Label::All, false);
        assert!(prefs.shown(&Label::All));
        prefs.set_shown(&Label::Drums, false);
        assert!(!prefs.shown(&Label::Drums));
        prefs.set_shown(&Label::Drums, true);
        assert!(prefs.shown(&Label::Drums));
    }
}
