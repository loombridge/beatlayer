//! The detail area's state (WP-6.27): which of the Clip View and the Device
//! View show, the view shown alone last, what Cmd+Option+L hid, the Clip
//! View's height, whether it's expanded, and which view has the keyboard
//! focus. It is view state: no part of the set, no undo step, and a window
//! holds one of its own. The behaviour is in the detail views spec, R1 to
//! R6, R10, R11 and R16.

use bl_shell_spec::commands::{CLIP_VIEW_HEIGHT, CLIP_VIEW_MIN_HEIGHT, DetailState, DetailView};

/// The tallest height `view.set_detail_height` keeps, which no window shows.
const MAX_HEIGHT: f64 = 4000.0;

/// The detail area of one window.
#[derive(Clone, Debug, PartialEq)]
pub struct DetailArea {
    clip: bool,
    device: bool,
    alone_last: DetailView,
    hidden: Vec<DetailView>,
    clip_height: f64,
    expanded: bool,
    focus: DetailView,
}

impl Default for DetailArea {
    /// A new session starts with the Device View alone (R16).
    fn default() -> Self {
        Self {
            clip: false,
            device: true,
            alone_last: DetailView::Device,
            hidden: Vec::new(),
            clip_height: f64::from(CLIP_VIEW_HEIGHT),
            expanded: false,
            focus: DetailView::Device,
        }
    }
}

impl DetailArea {
    /// Whether the Clip View shows.
    #[must_use]
    pub fn clip_shown(&self) -> bool {
        self.clip
    }

    /// Whether the Device View shows.
    #[must_use]
    pub fn device_shown(&self) -> bool {
        self.device
    }

    /// The Clip View's height in points at 100%, as it was set.
    #[must_use]
    pub fn clip_height(&self) -> f64 {
        self.clip_height
    }

    /// Whether the Clip View is expanded to its largest height.
    #[must_use]
    pub fn expanded(&self) -> bool {
        self.expanded
    }

    /// Whether `view` shows.
    #[must_use]
    pub fn shows(&self, view: DetailView) -> bool {
        self.is_shown(view)
    }

    /// Whether either view shows.
    #[must_use]
    pub fn any_shown(&self) -> bool {
        self.clip || self.device
    }

    /// The view the detail area's keys go to while both show.
    #[must_use]
    pub fn focus(&self) -> DetailView {
        self.focus
    }

    /// Whether the detail area's keys reach the Clip View: it shows alone,
    /// or it shows with the Device View and has the focus (R15).
    #[must_use]
    pub fn focus_in_clip(&self) -> bool {
        self.clip && (!self.device || self.focus == DetailView::Clip)
    }

    /// The views showing, and the Clip View's height, as a command reports
    /// them.
    #[must_use]
    pub fn state(&self) -> DetailState {
        DetailState {
            clip_view: self.clip,
            device_view: self.device,
            alone_last: self.alone_last,
            hidden: self.hidden.clone(),
            clip_height: self.clip_height,
            expanded: self.expanded,
            focus: self.focus,
        }
    }

    /// What `view.toggle_detail_area` would do: the views it would hide, or
    /// the ones it would bring back (R6).
    #[must_use]
    pub fn toggle_area_views(&self) -> (bool, Vec<DetailView>) {
        let showing: Vec<DetailView> = self.shown();
        if !showing.is_empty() {
            (true, showing)
        } else if self.hidden.is_empty() {
            (false, vec![DetailView::Device])
        } else {
            (false, self.hidden.clone())
        }
    }

    fn shown(&self) -> Vec<DetailView> {
        [
            (self.clip, DetailView::Clip),
            (self.device, DetailView::Device),
        ]
        .into_iter()
        .filter_map(|(on, view)| on.then_some(view))
        .collect()
    }

    fn set(&mut self, view: DetailView, on: bool) {
        match view {
            DetailView::Clip => self.clip = on,
            DetailView::Device => self.device = on,
        }
    }

    fn is_shown(&self, view: DetailView) -> bool {
        match view {
            DetailView::Clip => self.clip,
            DetailView::Device => self.device,
        }
    }

    /// Settles what a change leaves: the view shown alone is the one to
    /// bring back, the focus is in a view that shows, and hiding the Clip
    /// View ends its expansion (R5, R11, R16).
    fn settle(&mut self) {
        match (self.clip, self.device) {
            (true, false) => self.alone_last = DetailView::Clip,
            (false, true) => self.alone_last = DetailView::Device,
            _ => {}
        }
        if !self.is_shown(self.focus) {
            if self.clip {
                self.focus = DetailView::Clip;
            } else if self.device {
                self.focus = DetailView::Device;
            }
        }
        if !self.clip {
            self.expanded = false;
        }
    }

    /// Shows `view` if it's hidden and hides it if it shows, leaving the
    /// other as it is (R1). A view shown this way has the focus.
    pub fn toggle(&mut self, view: DetailView) {
        self.hidden.clear();
        if self.is_shown(view) {
            self.set(view, false);
        } else {
            self.set(view, true);
            self.focus = view;
        }
        self.settle();
    }

    /// A detail area showing neither view, as the second window opens with
    /// (second-window R7).
    #[must_use]
    pub fn empty() -> Self {
        let mut area = Self {
            device: false,
            ..Self::default()
        };
        area.settle();
        area
    }

    /// Takes the views `other` shows into this detail area, where they show
    /// as they did, with the Clip View's height and whether it's expanded,
    /// and leaves `other` showing neither (second-window R8, R9).
    pub fn take_from(&mut self, other: &mut Self) {
        self.hidden.clear();
        other.hidden.clear();
        for view in other.shown() {
            if view == DetailView::Clip {
                self.clip_height = other.clip_height;
                self.expanded = other.expanded;
            }
            self.set(view, true);
            other.set(view, false);
            self.focus = view;
        }
        self.settle();
        other.settle();
    }

    /// Hides `view` because it shows in the other window now
    /// (second-window R7).
    pub fn leave(&mut self, view: DetailView) {
        self.hidden.retain(|hidden| *hidden != view);
        self.set(view, false);
        self.settle();
    }

    /// Shows `view` alone, as a click on its selector does (R3).
    pub fn only(&mut self, view: DetailView) {
        self.hidden.clear();
        self.clip = view == DetailView::Clip;
        self.device = view == DetailView::Device;
        self.focus = view;
        self.settle();
    }

    /// Shows both views (R2).
    pub fn both(&mut self) {
        self.hidden.clear();
        self.clip = true;
        self.device = true;
        self.settle();
    }

    /// Shows `view` if it's hidden and gives it the focus (R14).
    pub fn focus_view(&mut self, view: DetailView) {
        self.hidden.clear();
        self.set(view, true);
        self.focus = view;
        self.settle();
    }

    /// Shift+Tab and F12 (R5): the other view in place of the one that
    /// shows; the focus to the other view while both show; the view shown
    /// alone last while neither does. Whether the focus moved between two
    /// showing views, which is the keyboard's to follow.
    pub fn switch(&mut self) -> bool {
        match (self.clip, self.device) {
            (true, true) => {
                self.hidden.clear();
                self.focus = match self.focus {
                    DetailView::Clip => DetailView::Device,
                    DetailView::Device => DetailView::Clip,
                };
                true
            }
            (true, false) => {
                self.only(DetailView::Device);
                false
            }
            (false, true) => {
                self.only(DetailView::Clip);
                false
            }
            (false, false) => {
                self.only(self.alone_last);
                false
            }
        }
    }

    /// Cmd+Option+L (R6): hides the views that show, and brings the same
    /// ones back; the Device View where nothing was hidden this way.
    pub fn toggle_area(&mut self) {
        let (hiding, views) = self.toggle_area_views();
        if hiding {
            self.hidden = views;
            self.clip = false;
            self.device = false;
        } else {
            self.hidden.clear();
            for view in views {
                self.set(view, true);
            }
        }
        self.settle();
    }

    /// Sets the Clip View's height, in points at 100%, ending an expansion
    /// (R10).
    pub fn set_clip_height(&mut self, points: f64) {
        self.clip_height = points.clamp(f64::from(CLIP_VIEW_MIN_HEIGHT), MAX_HEIGHT);
        self.expanded = false;
    }

    /// Expands the Clip View to its largest height, or puts back the height
    /// it had (R11). `false` while the Clip View is hidden.
    pub fn toggle_expanded(&mut self) -> bool {
        if !self.clip {
            return false;
        }
        self.expanded = !self.expanded;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_session_shows_the_device_view_alone() {
        let area = DetailArea::default();
        assert!(area.device_shown() && !area.clip_shown());
        assert_eq!(area.state().alone_last, DetailView::Device);
    }

    #[test]
    fn hiding_what_shows_remembers_it_and_a_change_forgets_it() {
        let mut area = DetailArea::default();
        area.both();
        area.toggle_area();
        assert!(!area.clip_shown() && !area.device_shown());
        assert_eq!(area.state().hidden, [DetailView::Clip, DetailView::Device]);
        area.toggle_area();
        assert!(area.clip_shown() && area.device_shown());
        area.toggle_area();
        area.toggle(DetailView::Clip);
        area.toggle(DetailView::Clip);
        // Nothing was hidden this way any more: the Device View comes.
        area.toggle_area();
        assert!(area.device_shown() && !area.clip_shown());
    }
}
