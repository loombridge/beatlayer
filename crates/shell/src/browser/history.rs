//! The browser's history (spec: browser-tags R29 to R31): the steps Browse
//! Back and Browse Forward go through.

use std::collections::BTreeSet;
use std::path::PathBuf;

use bl_shell_spec::browser::Label;

use super::{BrowserError, BrowserView};

/// How many steps the history holds (spec: browser-tags R29, Beatlayer's
/// own limit).
pub const LIMIT: usize = 100;

/// What the browser showed at one step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// The chosen label.
    pub label: Label,
    /// The search words.
    pub search: String,
    /// The selected filter tags.
    pub tags: BTreeSet<i64>,
    /// The open folders.
    pub open: BTreeSet<PathBuf>,
    /// The selection.
    pub selected: Vec<PathBuf>,
}

/// The steps taken this session, and where the browser is among them.
#[derive(Clone, Debug)]
pub struct History {
    steps: Vec<Step>,
    at: usize,
    /// Whether the present step began with a search and the search is all
    /// that has changed since, so more typing is the same step.
    search_run: bool,
}

impl History {
    /// A history that holds the first step.
    #[must_use]
    pub fn new(first: Step) -> Self {
        Self {
            steps: vec![first],
            at: 0,
            search_run: false,
        }
    }

    /// Whether there is a step before the present one.
    #[must_use]
    pub fn can_back(&self) -> bool {
        self.at > 0
    }

    /// Whether there is a step after the present one.
    #[must_use]
    pub fn can_forward(&self) -> bool {
        self.at + 1 < self.steps.len()
    }

    /// How many steps are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether the history holds no step; it always holds one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Keeps the open folders and the selection of the present step, which
    /// change without making a step.
    pub fn keep(&mut self, open: &BTreeSet<PathBuf>, selected: &[PathBuf]) {
        if let Some(step) = self.steps.get_mut(self.at) {
            step.open.clone_from(open);
            step.selected = selected.to_vec();
        }
    }

    /// Takes a step if the label, the selected tags or the search words are
    /// not what the present step holds (spec: browser-tags R29). Changes
    /// to the search words in a row are one step; a step taken after going
    /// back drops the steps ahead.
    pub fn record(&mut self, now: Step) {
        let Some(present) = self.steps.get(self.at) else {
            return;
        };
        let label = present.label != now.label;
        let tags = present.tags != now.tags;
        let search = present.search != now.search;
        if !label && !tags && !search {
            return;
        }
        let only_search = search && !label && !tags;
        if only_search && self.search_run {
            self.steps[self.at] = now;
            return;
        }
        self.steps.truncate(self.at + 1);
        self.steps.push(now);
        self.at += 1;
        if self.steps.len() > LIMIT {
            self.steps.remove(0);
            self.at -= 1;
        }
        self.search_run = only_search;
    }

    /// Makes the next change to the search words a step of its own.
    pub fn end_run(&mut self) {
        self.search_run = false;
    }

    /// Goes one step back or forward and gives the step now present.
    pub fn go(&mut self, back: bool) -> Option<Step> {
        let at = if back {
            self.at.checked_sub(1)?
        } else {
            (self.at + 1 < self.steps.len()).then_some(self.at + 1)?
        };
        self.at = at;
        self.search_run = false;
        self.steps.get(at).cloned()
    }
}

impl BrowserView {
    /// What the browser shows now, as a step.
    fn step(&self) -> Step {
        Step {
            label: self.label.clone(),
            search: self.search.clone(),
            tags: self.filter.clone(),
            open: self.open.clone(),
            selected: self.selected.clone(),
        }
    }

    /// Keeps the open folders and the selection in the present step.
    pub(super) fn keep_step(&mut self) {
        self.history.keep(&self.open, &self.selected);
    }

    /// Takes a step if the label, the selected tags or the search words
    /// changed.
    pub(super) fn record_step(&mut self) {
        let now = self.step();
        self.history.record(now);
    }

    /// Whether Browse Back can go back a step.
    #[must_use]
    pub fn can_back(&self) -> bool {
        self.history.can_back()
    }

    /// Whether Browse Forward can go forward a step.
    #[must_use]
    pub fn can_forward(&self) -> bool {
        self.history.can_forward()
    }

    /// Goes a step back or forward and shows it (spec: browser-tags R30).
    pub(super) fn go_step(&mut self, back: bool) -> Result<(), BrowserError> {
        self.keep_step();
        let Some(step) = self.history.go(back) else {
            return Err(BrowserError::Unavailable(
                if back {
                    "the browser is at its first step"
                } else {
                    "the browser is at its last step"
                }
                .to_owned(),
            ));
        };
        let exists = match &step.label {
            Label::Folder(path) => self
                .folders
                .iter()
                .any(|folder| folder.to_string_lossy() == path.as_str()),
            Label::Saved(id) => self.saved.iter().any(|saved| saved.id == *id),
            Label::Collection(number) => self.collections.exists(*number),
            _ => true,
        };
        self.label = if exists { step.label } else { Label::All };
        self.search = step.search;
        self.filter = step.tags;
        self.forget_gone_tags();
        self.open = step.open;
        self.selected = step.selected;
        self.rebuild();
        Ok(())
    }

    /// Hot-swapping started: the search words and the selected tags are
    /// cleared as a new step, so Browse Back brings them back (spec:
    /// browser-tags R31).
    pub fn hot_swap_started(&mut self) {
        self.keep_step();
        self.search.clear();
        self.filter.clear();
        self.rebuild();
        self.history.end_run();
        self.record_step();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(label: Label, search: &str, tags: &[i64]) -> Step {
        Step {
            label,
            search: search.to_owned(),
            tags: tags.iter().copied().collect(),
            open: BTreeSet::new(),
            selected: Vec::new(),
        }
    }

    // Spec: browser-tags R29, R30.
    #[test]
    fn typing_is_one_step_and_a_new_step_drops_those_ahead() {
        let mut history = History::new(step(Label::All, "", &[]));
        assert!(!history.can_back() && !history.can_forward());
        history.record(step(Label::Samples, "", &[]));
        for typed in ["k", "ki", "kic", "kick"] {
            history.record(step(Label::Samples, typed, &[]));
        }
        assert_eq!(history.len(), 3, "a label, then one search");
        history.record(step(Label::Samples, "kick", &[7]));
        assert_eq!(history.len(), 4);
        let back = history.go(true).unwrap();
        assert_eq!((back.search.as_str(), back.tags.len()), ("kick", 0));
        let back = history.go(true).unwrap();
        assert_eq!((back.label, back.search.as_str()), (Label::Samples, ""));
        assert_eq!(history.go(true).unwrap().label, Label::All);
        assert!(history.go(true).is_none() && !history.can_back());
        assert!(history.can_forward());
        history.go(false);
        history.record(step(Label::Drums, "", &[]));
        assert!(!history.can_forward(), "the steps ahead are gone");
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn the_history_holds_a_hundred_steps() {
        let mut history = History::new(step(Label::All, "", &[]));
        for tag in 0..150 {
            history.record(step(Label::All, "", &[tag + 1]));
        }
        assert_eq!(history.len(), LIMIT);
        assert!(history.can_back() && !history.can_forward());
    }

    #[test]
    fn typing_again_after_another_step_is_a_new_step() {
        let mut history = History::new(step(Label::All, "", &[]));
        history.record(step(Label::All, "a", &[]));
        history.record(step(Label::All, "a", &[1]));
        history.record(step(Label::All, "ab", &[1]));
        assert_eq!(history.len(), 4);
    }
}
