//! The second window and the commands that act on one window (WP-6.29; the
//! second-window spec R1 to R15 and the interface-zoom spec R11 to R15).
//!
//! The two windows always show different main views, so the shell keeps one
//! `main_view`, the main window's, and the second window shows the other.
//! The second window holds a browser switch, a detail area, a focused area
//! and a full-screen mode of its own; the Clip View and the Device View each
//! show in at most one window.

use crate::commands::{ViewResult, ZoomArgs};
use crate::settings::{zoom_factor, zoom_percent, zoom_step};
use bl_model::WindowViews;
use bl_shell_spec::commands::{DetailView, SecondWindowState, WindowArgs, WindowName};

use super::*;

/// The second window's own state. It is view state: the set keeps only
/// whether the window is open (R14).
#[derive(Debug)]
pub(super) struct SecondWindow {
    pub open: bool,
    pub browser: bool,
    pub detail: DetailArea,
    pub area: Area,
    pub full_screen: bool,
}

impl Default for SecondWindow {
    /// Closed; when it opens it has no browser and no detail view (R2, R7).
    fn default() -> Self {
        Self {
            open: false,
            browser: false,
            detail: DetailArea::empty(),
            area: Area::MainView,
            full_screen: false,
        }
    }
}

impl MainView {
    /// The other main view.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Arrangement => Self::Session,
            Self::Session => Self::Arrangement,
        }
    }
}

fn unavailable(command: &str, reason: &str) -> ShellError {
    ShellError::Unavailable {
        command: command.to_owned(),
        reason: reason.to_owned(),
    }
}

impl Shell {
    /// Whether the second window is open.
    #[must_use]
    pub fn second_window_open(&self) -> bool {
        self.second.open
    }

    /// The window the commands that act on one window act on when they
    /// aren't told which: the main window, or the second window while it is
    /// open and was in front last (second-window R10).
    #[must_use]
    pub fn acting_window(&self) -> WindowKind {
        if self.second.open {
            self.front_window
        } else {
            WindowKind::Main
        }
    }

    /// The main view `window` shows: the second window shows the one the
    /// main window doesn't (R4).
    #[must_use]
    pub fn main_view_of(&self, window: WindowKind) -> MainView {
        if window == WindowKind::Second {
            self.main_view.other()
        } else {
            self.main_view
        }
    }

    /// The window that shows `view`, the main window while the second is
    /// closed.
    #[must_use]
    pub fn window_showing(&self, view: MainView) -> WindowKind {
        if self.second.open && self.main_view != view {
            WindowKind::Second
        } else {
            WindowKind::Main
        }
    }

    /// Whether `window`'s browser shows (R5).
    #[must_use]
    pub fn browser_of(&self, window: WindowKind) -> bool {
        if window == WindowKind::Second {
            self.second.browser
        } else {
            self.browser
        }
    }

    /// Whether `window` shows the Info View: it belongs to the main window
    /// (R12).
    #[must_use]
    pub fn info_view_of(&self, window: WindowKind) -> bool {
        window != WindowKind::Second && self.info_view
    }

    /// `window`'s detail area.
    #[must_use]
    pub fn detail_of(&self, window: WindowKind) -> &DetailArea {
        if window == WindowKind::Second {
            &self.second.detail
        } else {
            &self.detail
        }
    }

    fn detail_mut(&mut self, window: WindowKind) -> &mut DetailArea {
        if window == WindowKind::Second {
            &mut self.second.detail
        } else {
            &mut self.detail
        }
    }

    /// The area of `window` with the keyboard focus.
    #[must_use]
    pub fn focused_area_of(&self, window: WindowKind) -> Area {
        if window == WindowKind::Second {
            self.second.area
        } else {
            self.focused_area
        }
    }

    /// Records which area of `window` has the keyboard focus, as clicking
    /// in it does (I-FOCUS-001).
    pub fn set_focused_area_in(&mut self, window: WindowKind, area: Area) {
        if window != WindowKind::Second {
            self.set_focused_area(area);
        } else if self.second.area != area {
            self.second.area = area;
            self.value_focused &= area == Area::Detail;
            self.touch();
        }
    }

    /// Whether `window` is in full-screen mode (R13).
    #[must_use]
    pub fn full_screen_of(&self, window: WindowKind) -> bool {
        if window == WindowKind::Second {
            self.second.full_screen
        } else {
            self.full_screen
        }
    }

    /// Records that the second window left full-screen mode by other means.
    pub fn set_second_full_screen_state(&mut self, on: bool) {
        if self.second.full_screen != on {
            self.second.full_screen = on;
            self.touch();
        }
    }

    /// The interface zoom of `window` as a whole percentage: the second
    /// window's own, which is the main window's until it has been set
    /// (interface-zoom R11).
    #[must_use]
    pub fn zoom_percent_of(&self, window: WindowKind) -> u16 {
        match (window, self.prefs.second_zoom) {
            (WindowKind::Second, Some(zoom)) => zoom_percent(zoom),
            _ => self.zoom_percent(),
        }
    }

    /// The interface zoom of `window`, where 1 is 100%.
    #[must_use]
    pub fn zoom_of(&self, window: WindowKind) -> f32 {
        zoom_factor(self.zoom_percent_of(window))
    }

    fn set_zoom_percent_of(&mut self, window: WindowKind, percent: u16) {
        if window != WindowKind::Second {
            self.set_zoom_percent(percent);
            return;
        }
        let zoom = Some(zoom_factor(percent));
        if self.prefs.second_zoom != zoom {
            self.prefs.second_zoom = zoom;
            self.save_preferences();
        }
    }

    /// Where the second window sat when it was last open (R1).
    #[must_use]
    pub fn second_window_frame(&self) -> Option<bl_model::WindowFrame> {
        self.prefs.second_window
    }

    /// Records where the second window sits and how big it is, kept for the
    /// next time it opens.
    pub fn set_second_window_frame(&mut self, frame: bl_model::WindowFrame) {
        if frame.is_valid() {
            self.prefs.second_window = Some(frame);
        }
    }

    /// What `window` shows of the view state the view commands return.
    pub(super) fn view_result_of(&self, window: WindowKind) -> ViewResult {
        ViewResult {
            main_view: self.main_view_of(window).id().to_owned(),
            browser: self.browser_of(window),
            // Spec: browser R26. The second window's browser keeps its height.
            full_height_browser: window == WindowKind::Main && self.browser_view.full_height(),
            tuning_section: self.tuning_section,
            browser_width: self.browser_view.width(),
            info_view: self.info_view_of(window),
            full_screen: self.full_screen_of(window),
            zoom: self.zoom_of(window),
        }
    }

    /// The second window's state, as `view.state` reports it (R15).
    pub(super) fn second_window_state(&self) -> SecondWindowState {
        SecondWindowState {
            open: self.second.open,
            main_view: self.main_view.other().id().to_owned(),
            browser: self.second.browser,
            full_screen: self.second.full_screen,
            zoom: self.zoom_of(WindowKind::Second),
            frame: self.prefs.second_window,
            detail: self.second.detail.state(),
        }
    }

    /// The window a command acts on: the one `name` says, or the one in
    /// front. The second window has to be open.
    fn target_window(&self, id: &str, name: Option<WindowName>) -> Result<WindowKind, ShellError> {
        match name {
            None => Ok(self.acting_window()),
            Some(WindowName::Main) => Ok(WindowKind::Main),
            Some(WindowName::Second) if self.second.open => Ok(WindowKind::Second),
            Some(WindowName::Second) => Err(unavailable(id, "the second window isn't open")),
        }
    }

    /// The window a command with `args` acts on, as far as can be told
    /// without running it: for the menus' and keys' enabled states.
    pub(super) fn window_in(&self, args: &Value) -> WindowKind {
        match args["window"].as_str() {
            Some("main") => WindowKind::Main,
            Some("second") if self.second.open => WindowKind::Second,
            _ => self.acting_window(),
        }
    }

    /// Records the main view and the second window's switch for the set to
    /// save beside the document (R14).
    pub(super) fn sync_window_views(&mut self) {
        self.store.set_window_views(WindowViews {
            session: self.main_view == MainView::Session,
            second_window: self.second.open,
        });
    }

    /// Shows `view` in the main window, and the other in the second.
    pub(super) fn show_main_view(&mut self, view: MainView) {
        self.main_view = view;
        self.sync_window_views();
    }

    /// Takes the main view and second window a set was saved with (R14).
    /// The window opens without taking the focus.
    pub(super) fn apply_window_views(&mut self, views: WindowViews) {
        self.main_view = if views.session {
            MainView::Session
        } else {
            MainView::Arrangement
        };
        if views.second_window && !self.second.open {
            self.open_second_window(false);
        } else if !views.second_window && self.second.open {
            self.close_second_window();
        }
        self.sync_window_views();
    }

    fn open_second_window(&mut self, focus: bool) {
        self.second = SecondWindow {
            open: true,
            ..SecondWindow::default()
        };
        // The first time, it opens at the main window's zoom (interface-zoom
        // R11).
        if self.prefs.second_zoom.is_none() {
            self.prefs.second_zoom = Some(self.zoom());
            self.save_preferences();
        }
        if focus {
            self.focused_window = WindowKind::Second;
            self.front_window = WindowKind::Second;
        }
    }

    fn close_second_window(&mut self) {
        // Its detail views come back to the main window (R9).
        self.detail.take_from(&mut self.second.detail);
        self.second.open = false;
        self.second.full_screen = false;
        // A drag of clips that began in it, or is over it, ends with it.
        if self
            .clip_drag
            .as_ref()
            .is_some_and(|drag| drag.window == WindowKind::Second)
        {
            self.clip_drag = None;
        }
        if let Some(drag) = &mut self.clip_drag {
            drag.over = drag
                .over
                .filter(|(window, _)| *window != WindowKind::Second);
        }
        if self.focused_window == WindowKind::Second {
            self.focused_window = WindowKind::Main;
        }
        self.front_window = WindowKind::Main;
        self.save_preferences();
        self.clip_view.set_visible(self.detail.clip_shown());
    }

    /// Keeps each of the Clip View and the Device View in one window: what
    /// `kept` shows leaves the other window (R7). The Clip View's own copy
    /// of whether it shows follows the detail areas, the one place every
    /// change passes.
    fn settle_detail(&mut self, kept: WindowKind) {
        if self.second.open {
            let other = if kept == WindowKind::Second {
                WindowKind::Main
            } else {
                WindowKind::Second
            };
            for view in [DetailView::Clip, DetailView::Device] {
                if self.detail_of(kept).shows(view) {
                    self.detail_mut(other).leave(view);
                }
            }
        }
        self.clip_view
            .set_visible(self.detail.clip_shown() || self.second.detail.clip_shown());
    }

    fn detail_result_of(&mut self, window: WindowKind) -> Value {
        self.settle_detail(window);
        self.touch();
        to_value(&self.detail_of(window).state())
    }

    /// Gives `window`'s detail area the keyboard focus, bringing the window
    /// to the front.
    fn focus_detail_in(&mut self, window: WindowKind) {
        self.focus_area_in(window, Area::Detail);
    }

    /// Gives `area` of `window` the keyboard focus, bringing the window to
    /// the front.
    pub(super) fn focus_area_in(&mut self, window: WindowKind, area: Area) {
        self.focused_window = window;
        if window == WindowKind::Second {
            self.front_window = window;
            self.second.area = area;
            self.effects.push(Effect::FocusSecondArea(area));
        } else {
            self.front_window = WindowKind::Main;
            self.focused_area = area;
            self.effects.push(Effect::FocusArea(area));
        }
    }

    /// Gives `area` the keyboard focus in the window in front.
    pub(super) fn focus_area(&mut self, area: Area) {
        self.focus_area_in(self.acting_window(), area);
    }

    /// Navigate ▸ Session View and Arrangement View (R11): the focus goes
    /// to the view in the window that shows it; with one window the view is
    /// shown first.
    pub(super) fn focus_main_view(&mut self, view: MainView) {
        if !self.second.open {
            self.show_main_view(view);
        }
        self.focus_area_in(self.window_showing(view), Area::MainView);
    }

    /// Shows `window`'s browser, as loading from it does.
    pub(super) fn show_browser_in(&mut self, window: WindowKind) {
        if window == WindowKind::Second {
            self.second.browser = true;
        } else {
            self.browser = true;
        }
    }

    /// F12, Shift+Tab and Toggle Clip/Device View with two windows open
    /// (R8): the views the window in front shows move to the other
    /// window, or, when it shows neither, the other window's come to it.
    fn move_detail_views(&mut self, from: WindowKind) {
        let other = if from == WindowKind::Second {
            WindowKind::Main
        } else {
            WindowKind::Second
        };
        let target = if self.detail_of(from).any_shown() {
            other
        } else {
            from
        };
        if target == WindowKind::Second {
            self.second.detail.take_from(&mut self.detail);
        } else {
            self.detail.take_from(&mut self.second.detail);
        }
    }

    /// Runs the view commands that act on one window, and the second
    /// window's own (R15, interface-zoom R14).
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per command keeps each one's behaviour in sight"
    )]
    pub(super) fn run_window_view(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::VIEW_TOGGLE_SECOND_WINDOW => {
                Self::args::<NoArgs>(id, args)?;
                if self.second.open {
                    self.close_second_window();
                } else {
                    self.open_second_window(true);
                }
                self.sync_window_views();
                self.touch();
                Ok(to_value(&self.second_window_state()))
            }
            ids::VIEW_TOGGLE_SESSION_ARRANGEMENT => {
                Self::args::<NoArgs>(id, args)?;
                self.show_main_view(self.main_view.other());
                Ok(to_value(&self.view_result_of(self.acting_window())))
            }
            ids::FOCUS_ARRANGEMENT => {
                Self::args::<NoArgs>(id, args)?;
                self.focus_main_view(MainView::Arrangement);
                Ok(Value::Null)
            }
            ids::FOCUS_CONTROL_BAR => {
                Self::args::<NoArgs>(id, args)?;
                self.focus_area(Area::ControlBar);
                Ok(Value::Null)
            }
            ids::VIEW_FULL_SCREEN => {
                let WindowArgs { window } = Self::args(id, args)?;
                let window = self.target_window(id, window)?;
                if window == WindowKind::Second {
                    self.second.full_screen = !self.second.full_screen;
                    self.effects
                        .push(Effect::SetSecondFullScreen(self.second.full_screen));
                } else {
                    self.full_screen = !self.full_screen;
                    self.effects.push(Effect::SetFullScreen(self.full_screen));
                }
                Ok(to_value(&self.view_result_of(window)))
            }
            ids::VIEW_TOGGLE_BROWSER => {
                let WindowArgs { window } = Self::args(id, args)?;
                let window = self.target_window(id, window)?;
                let shown = !self.browser_of(window);
                if window == WindowKind::Second {
                    self.second.browser = shown;
                } else {
                    self.browser = shown;
                }
                if !shown && self.focused_area_of(window) == Area::Browser {
                    self.set_focused_area_in(window, Area::MainView);
                }
                Ok(to_value(&self.view_result_of(window)))
            }
            ids::VIEW_TOGGLE_CLIP_VIEW | ids::VIEW_TOGGLE_DEVICE_VIEW => {
                // Spec: detail-views R1; L-STAT-004, L-STAT-006. View state
                // only.
                let WindowArgs { window } = Self::args(id, args)?;
                let window = self.target_window(id, window)?;
                let view = if id == ids::VIEW_TOGGLE_CLIP_VIEW {
                    DetailView::Clip
                } else {
                    DetailView::Device
                };
                self.detail_mut(window).toggle(view);
                Ok(self.detail_result_of(window))
            }
            ids::VIEW_SHOW_DETAIL_VIEWS => {
                // Spec: detail-views R2; I-CLICK-027.
                let WindowArgs { window } = Self::args(id, args)?;
                let window = self.target_window(id, window)?;
                self.detail_mut(window).both();
                Ok(self.detail_result_of(window))
            }
            ids::VIEW_SWITCH_DETAIL_VIEW => {
                // Spec: detail-views R5; second-window R8.
                let WindowArgs { window } = Self::args(id, args)?;
                let window = self.target_window(id, window)?;
                if self.second.open && self.detail.any_shown() || self.second.detail.any_shown() {
                    self.move_detail_views(window);
                } else if self.detail_mut(window).switch() {
                    self.focus_detail_in(window);
                }
                Ok(self.detail_result_of(window))
            }
            ids::VIEW_TOGGLE_DETAIL_AREA => {
                // Spec: detail-views R6; S-VIEW-022.
                let WindowArgs { window } = Self::args(id, args)?;
                let window = self.target_window(id, window)?;
                self.detail_mut(window).toggle_area();
                Ok(self.detail_result_of(window))
            }
            ids::VIEW_EXPAND_CLIP_VIEW => {
                // Spec: detail-views R11; S-NOTE-030.
                let WindowArgs { window } = Self::args(id, args)?;
                let window = self.target_window(id, window)?;
                if !self.detail_mut(window).toggle_expanded() {
                    return Err(unavailable(id, "the Clip View is hidden"));
                }
                Ok(self.detail_result_of(window))
            }
            ids::VIEW_SET_DETAIL_HEIGHT => {
                // Spec: detail-views R10, R18.
                let ids::DetailHeightArgs { points, window } = Self::args(id, args)?;
                let window = self.target_window(id, window)?;
                if !points.is_finite() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "`points` must be a number".to_owned(),
                    });
                }
                self.detail_mut(window).set_clip_height(points);
                Ok(self.detail_result_of(window))
            }
            ids::FOCUS_CLIP_VIEW | ids::FOCUS_DEVICE_VIEW => {
                // Spec: detail-views R14; second-window R11. The focus goes
                // to the window that shows the view, or the one in front
                // when neither does.
                let WindowArgs { window } = Self::args(id, args)?;
                let view = if id == ids::FOCUS_CLIP_VIEW {
                    DetailView::Clip
                } else {
                    DetailView::Device
                };
                let window = if window.is_some() {
                    self.target_window(id, window)?
                } else if self.second.detail.shows(view) {
                    WindowKind::Second
                } else if self.detail.shows(view) {
                    WindowKind::Main
                } else {
                    self.acting_window()
                };
                self.detail_mut(window).focus_view(view);
                self.focus_detail_in(window);
                Ok(self.detail_result_of(window))
            }
            ids::VIEW_ZOOM_IN | ids::VIEW_ZOOM_OUT | ids::VIEW_ZOOM_RESET => {
                // Spec: interface-zoom R3, R5, R6, R13, R14.
                let WindowArgs { window } = Self::args(id, args)?;
                let window = self.zoom_window(window);
                let current = self.zoom_percent_of(window);
                let next = match id {
                    ids::VIEW_ZOOM_IN => zoom_step(current, 1),
                    ids::VIEW_ZOOM_OUT => zoom_step(current, -1),
                    _ => 100,
                };
                self.set_zoom_percent_of(window, next);
                Ok(to_value(&self.view_result_of(window)))
            }
            ids::VIEW_SET_ZOOM => {
                let ZoomArgs { percent, window } = Self::args(id, args)?;
                let window = self.zoom_window(window);
                self.set_zoom_percent_of(window, percent);
                Ok(to_value(&self.view_result_of(window)))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// The window a zoom command acts on: `second` names the second window
    /// even while it is closed, since it sets the zoom it opens at
    /// (interface-zoom R14).
    pub(super) fn zoom_window(&self, name: Option<WindowName>) -> WindowKind {
        match name {
            Some(WindowName::Second) => WindowKind::Second,
            Some(WindowName::Main) => WindowKind::Main,
            None => self.acting_window(),
        }
    }
}
