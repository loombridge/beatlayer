//! The list of the set's plug-ins (IMP-29, WP-6.41): whether it shows in
//! the detail area's place, and whether Show Only Missing is on. View
//! state: it isn't saved with the set and isn't an undo step.

use bl_shell_spec::plugins::PluginListState;

/// What the list of a set's plug-ins shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PluginList {
    /// Whether the list shows in the detail area's place (L-OTH-002).
    visible: bool,
    /// Whether Show Only Missing is on, so that only the plug-ins this
    /// computer doesn't have show.
    only_missing: bool,
}

impl PluginList {
    /// Whether the list shows.
    #[must_use]
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Whether Show Only Missing is on.
    #[must_use]
    pub fn only_missing(&self) -> bool {
        self.only_missing
    }

    /// Shows the list, or hides it again. Opening it turns Show Only
    /// Missing off, as the list's switch starts each time it opens.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            self.only_missing = false;
        }
    }

    /// Switches Show Only Missing.
    pub fn set_only_missing(&mut self, only_missing: bool) {
        self.only_missing = only_missing;
    }

    /// What the list shows, as `view.state` reports it.
    #[must_use]
    pub fn state(&self) -> PluginListState {
        PluginListState {
            visible: self.visible,
            only_missing: self.only_missing,
        }
    }
}
