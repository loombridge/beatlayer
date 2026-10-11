//! The main window: the Control Bar, the browser, the main view, the detail
//! area, and the Status Bar, placed by the layout layer (L-WIN, L-CTRL,
//! L-STAT). The main view shows the Arrangement View (WP-1.8a); views that
//! later packages build are placeholders that say so.

use std::time::Duration;

use bl_shell::{Area, Effect, GesturePhase, Invocation, MainView, Shell, commands as ids};
use egui::accesskit::Role;
use egui::{
    Align2, CentralPanel, Context, Frame, Id, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder,
};
use serde_json::json;

use crate::arrangement::ArrangementUi;
use crate::control_bar::{self, TransportFields};
use crate::layout::{ELEMENTS, Element, Kind, ShellLayout, Strip, title_bar_height};
use crate::theme::Theme;
use crate::widgets::{Look, element};

/// What the person did in a frame, for the host to carry out.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// Run a command, as [`Shell::invoke`] does.
    Invoke(Invocation),
    /// The person clicked in an area, which takes the keyboard focus
    /// (I-FOCUS-001).
    FocusArea(Area),
    /// The Arrangement's track lanes are laid out at a new size, in points,
    /// for the host to pass to [`Shell::set_arrangement_viewport`].
    ArrangementViewport {
        /// From the lanes' left edge to the track headers.
        width: f32,
        /// From under the rulers to the pinned return tracks.
        height: f32,
    },
    /// A step of a gesture that edits the set, such as dragging clips
    /// (WP-1.8b), for the host to pass to [`Shell::gesture`], which runs
    /// the whole gesture as one undo step.
    Gesture {
        /// Whether the gesture goes on, ends, or is abandoned.
        phase: GesturePhase,
        /// The editing command for this step, measured from where the
        /// gesture began.
        invocation: Option<Invocation>,
    },
    /// A drag on a value began: the document commands it runs until
    /// [`UiEvent::EndGesture`] are one undo step with this name, as
    /// [`Shell::begin_gesture`] makes them.
    BeginGesture(&'static str),
    /// The drag ended ([`Shell::end_gesture`]).
    EndGesture,
}

/// The main window's UI state between frames.
#[derive(Debug)]
pub struct MainWindowUi {
    /// A focus move asked for by a command, done in the next frame.
    focus_request: Option<FocusRequest>,
    /// The Control Bar control focused last, which focus returns to
    /// (I-FOCUS-003).
    last_control: Element,
    /// The element under the pointer, for the Info View.
    hovered: Option<Element>,
    /// A move to the next or previous control, done at the start of the
    /// next frame (egui forgets one asked for between frames).
    move_focus: Option<egui::FocusDirection>,
    /// The Arrangement View's caches and gestures (WP-1.8a).
    arrangement: ArrangementUi,
    /// The Control Bar's transport fields: a value being typed, a drag
    /// (WP-1.18).
    transport: TransportFields,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum FocusRequest {
    Element(Element),
    MainView,
}

impl Default for MainWindowUi {
    fn default() -> Self {
        Self::new()
    }
}

/// The ID of the main view's focusable area.
#[must_use]
pub fn main_view_id() -> Id {
    Id::new("main-view")
}

/// The ID of an element's widget.
#[must_use]
pub fn element_id(element: Element) -> Id {
    Id::new(("element", element))
}

impl MainWindowUi {
    /// A fresh main window.
    #[must_use]
    pub fn new() -> Self {
        Self {
            focus_request: None,
            last_control: Element::BrowserToggle,
            hovered: None,
            move_focus: None,
            arrangement: ArrangementUi::new(),
            transport: TransportFields::new(),
        }
    }

    /// Whether a value is being typed into a Control Bar field: while it
    /// is, key presses go to the field rather than to shortcuts, as they do
    /// to a text field (the control bar transport spec, R10).
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.transport.is_typing()
    }

    /// The Arrangement View, to give it a peak source or read what it drew.
    pub fn arrangement_mut(&mut self) -> &mut ArrangementUi {
        &mut self.arrangement
    }

    /// The Arrangement View.
    #[must_use]
    pub fn arrangement(&self) -> &ArrangementUi {
        &self.arrangement
    }

    /// Carries out an effect that moves the keyboard focus in this window.
    pub fn apply_effect(&mut self, ctx: &Context, effect: &Effect) {
        match effect {
            Effect::FocusArea(Area::ControlBar) => {
                self.focus_request = Some(FocusRequest::Element(self.last_control));
            }
            Effect::FocusArea(Area::MainView) => self.focus_request = Some(FocusRequest::MainView),
            Effect::MoveFocus { backwards } => self.move_focus = Some(direction(*backwards)),
            _ => {}
        }
        ctx.request_repaint();
    }

    /// Draws the main window and returns what the person did. With
    /// `title_bar`, it draws Live's 23-point title bar along the top, for a
    /// window whose content covers the system's title bar (macOS, D22);
    /// with `menu_bar`, the menu bar under it, for platforms without a
    /// native one.
    pub fn show(
        &mut self,
        root: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        title_bar: bool,
        menu_bar: Option<DrawnMenuBar<'_>>,
    ) -> Vec<UiEvent> {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        if let Some(direction) = self.move_focus.take() {
            ctx.memory_mut(|m| m.move_focus(direction));
        }
        let mut events = Vec::new();
        if title_bar {
            draw_title_bar(root, theme, &shell.set_name());
        }
        if let Some(bar) = menu_bar {
            egui::Panel::top("menu-bar")
                .frame(Frame::NONE.fill(theme.colors.bar.0).inner_margin(2.0))
                .show(root, |ui| {
                    crate::menus::menu_bar(
                        ui,
                        bar.menus,
                        shell,
                        bar.keymap,
                        bar.platform,
                        &mut events,
                    );
                });
        }
        self.hovered = None;
        let frame = Frame::NONE.fill(theme.colors.window.0);
        CentralPanel::default().frame(frame).show(root, |ui| {
            let area = ui.max_rect();
            let layout = ShellLayout::compute(area.size(), shell.browser_visible())
                .translate(area.min.to_vec2());
            self.track_area_clicks(ctx, &layout, &mut events);
            self.control_bar(ui, shell, theme, &layout, &mut events);
            if let Some(browser) = layout.browser {
                placeholder(
                    ui,
                    theme,
                    "region-browser",
                    browser,
                    Role::Group,
                    "Browser",
                    "The browser arrives with WP-1.10.",
                );
            }
            self.main_view(ui, shell, theme, layout.main_view, &mut events);
            self.info_view(ui, theme, layout.info_view);
            placeholder(
                ui,
                theme,
                "region-device-view",
                layout.device_view,
                Role::Group,
                "Device View",
                "No devices on this track. The Device View arrives with WP-2.4.",
            );
            self.status_bar(ui, shell, theme, &layout, &mut events);
        });
        if shell.playing() {
            // The playhead and the Arrangement Position fields move with
            // the engine every frame (control-bar-transport R14).
            ctx.request_repaint();
        } else if shell.audio().engine_on() {
            // The CPU meter updates a few times a second.
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        events
    }

    fn track_area_clicks(&self, ctx: &Context, layout: &ShellLayout, events: &mut Vec<UiEvent>) {
        let Some(pos) = ctx.input(|i| {
            i.pointer
                .any_pressed()
                .then(|| i.pointer.press_origin())
                .flatten()
        }) else {
            return;
        };
        let areas = [
            (Area::ControlBar, Some(layout.control_bar)),
            (Area::Browser, layout.browser),
            (Area::MainView, Some(layout.main_view)),
            (Area::Detail, Some(layout.detail)),
            (Area::StatusBar, Some(layout.status_bar)),
        ];
        if let Some((area, _)) = areas
            .iter()
            .find(|(_, rect)| rect.is_some_and(|r| r.contains(pos)))
        {
            events.push(UiEvent::FocusArea(*area));
        }
    }

    fn strip(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &ShellLayout,
        strip: Strip,
        events: &mut Vec<UiEvent>,
    ) {
        for (placement, (el, rect)) in ELEMENTS.iter().zip(&layout.elements) {
            if placement.strip != strip {
                continue;
            }
            let look = self
                .transport
                .look(shell, *el)
                .unwrap_or_else(|| look(*el, shell));
            let response = element(ui, theme, placement, *rect, &look);
            if self.focus_request == Some(FocusRequest::Element(*el)) {
                response.request_focus();
                self.focus_request = None;
            }
            if placement.pending.is_none()
                && let Some(field) = control_bar::field(*el)
            {
                self.transport.interact(ui, shell, field, &response, events);
            }
            if response.has_focus() && strip == Strip::ControlBar {
                self.last_control = *el;
            }
            if response.hovered() {
                self.hovered = Some(*el);
            }
            if response.clicked()
                && let Some(invocation) = click(*el, shell)
            {
                events.push(UiEvent::Invoke(invocation));
            }
        }
    }

    fn control_bar(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &ShellLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let id = region(
            ui,
            "region-control-bar",
            layout.control_bar,
            Role::Toolbar,
            "Control Bar",
        );
        ui.painter()
            .rect_filled(layout.control_bar, 0.0, theme.colors.bar.0);
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(layout.control_bar)
                .accessibility_parent(id),
            |ui| self.strip(ui, shell, theme, layout, Strip::ControlBar, events),
        );
        self.transport.end_frame(ui);
    }

    fn status_bar(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &ShellLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let id = region(
            ui,
            "region-status-bar",
            layout.status_bar,
            Role::Group,
            "Status Bar",
        );
        ui.painter()
            .rect_filled(layout.status_bar, 0.0, theme.colors.bar.0);
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(layout.status_bar)
                .accessibility_parent(id),
            |ui| self.strip(ui, shell, theme, layout, Strip::StatusBar, events),
        );
    }

    fn main_view(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let (name, note) = match shell.main_view() {
            MainView::Arrangement => (
                "Arrangement View",
                "Tracks and their clips along the song's timeline, with the rulers above and \
                 the track headers at the right.",
            ),
            MainView::Session => (
                "Session View",
                "The Session View's clip slots and scenes arrive in milestone M3.",
            ),
        };
        let response = ui.interact(rect, main_view_id(), Sense::click());
        let pressed_here = ui.input(|i| {
            i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|p| rect.contains(p))
        });
        if self.focus_request == Some(FocusRequest::MainView) || pressed_here {
            // A click in the view gives it the keyboard (I-FOCUS-001).
            response.request_focus();
            self.focus_request = None;
        }
        match shell.main_view() {
            MainView::Arrangement => self.arrangement.show(ui, shell, theme, rect, events),
            MainView::Session => draw_pane(ui, theme, rect, name, note),
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                0.0,
                Stroke::new(1.0, theme.colors.focus.0),
                StrokeKind::Inside,
            );
        }
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Panel, true, name));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Pane);
            node.set_description(note.to_owned());
        });
    }

    fn info_view(&self, ui: &mut Ui, theme: &Theme, rect: Rect) {
        let id = region(ui, "region-info-view", rect, Role::Group, "Info View");
        let _ = id;
        ui.painter().rect_filled(rect, 0.0, theme.colors.panel.0);
        // Like Live's Info View, it describes the element under the pointer
        // (L-INFO-002, I-FOCUS-012).
        let (title, body) = match self.hovered.map(crate::layout::placement) {
            Some(placement) => (
                placement.name.to_owned(),
                match placement.pending {
                    Some(package) => format!("Not built yet: arrives with {package}."),
                    None => "Point at a control to read about it here.".to_owned(),
                },
            ),
            None => (
                "Info View".to_owned(),
                "Point at a control to read about it here. The full Info View arrives in milestone M2."
                    .to_owned(),
            ),
        };
        let painter = ui.painter_at(rect);
        let mut y = rect.top() + 8.0;
        painter.text(
            egui::pos2(rect.left() + 8.0, y),
            Align2::LEFT_TOP,
            title,
            egui::FontId::proportional(theme.sizes.heading),
            theme.colors.text.0,
        );
        y += 20.0;
        let galley = painter.layout(
            body,
            theme.small_font(),
            theme.colors.text_dim.0,
            rect.width() - 16.0,
        );
        painter.galley(
            egui::pos2(rect.left() + 8.0, y),
            galley,
            theme.colors.text_dim.0,
        );
    }
}

/// Draws Live's compact title bar across the top of a window (L-WIN-002,
/// D22): [`crate::layout::TITLE_BAR`] points tall, the title centred. The
/// system's window buttons sit over its left end, and the system's title bar
/// view over all of it handles dragging and double-clicks, so this only
/// paints. The title reaches assistive technology as the window's own title.
///
/// At every interface zoom it stays that many window points tall, with its
/// title at its 100% size, since the system's buttons in it don't scale
/// (interface-zoom R9).
pub fn draw_title_bar(root: &mut Ui, theme: &Theme, title: &str) {
    let zoom = root.ctx().zoom_factor();
    let font = theme.font();
    egui::Panel::top("title-bar")
        .exact_size(title_bar_height(zoom))
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::NONE.fill(theme.colors.bar.0))
        .show(root, |ui| {
            let rect = ui.max_rect();
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                title,
                egui::FontId::new(font.size / zoom, font.family),
                theme.colors.text_dim.0,
            );
            ui.painter().line_segment(
                [rect.left_bottom(), rect.right_bottom()],
                Stroke::new(1.0 / zoom, theme.colors.separator.0),
            );
        });
}

/// The focus direction for a move to the next or previous control.
#[must_use]
pub fn direction(backwards: bool) -> egui::FocusDirection {
    if backwards {
        egui::FocusDirection::Previous
    } else {
        egui::FocusDirection::Next
    }
}

/// An accessible region: a node with a role and a name that the elements
/// inside it hang from. Returns its ID.
fn region(ui: &mut Ui, salt: &str, rect: Rect, role: Role, name: &str) -> Id {
    let id = Id::new(salt);
    let _ = ui.interact(rect, id, Sense::hover());
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(role);
        node.set_label(name.to_owned());
        // Its place, so a screen reader can show where the region is (and
        // the zoom tests can find it).
        node.set_bounds(egui::accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
    id
}

fn draw_pane(ui: &Ui, theme: &Theme, rect: Rect, name: &str, note: &str) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme.colors.panel.0);
    painter.text(
        rect.center() - egui::vec2(0.0, 10.0),
        Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(theme.sizes.heading),
        theme.colors.text_dim.0,
    );
    painter.text(
        rect.center() + egui::vec2(0.0, 10.0),
        Align2::CENTER_CENTER,
        note,
        theme.small_font(),
        theme.colors.pending.0,
    );
}

fn placeholder(
    ui: &mut Ui,
    theme: &Theme,
    salt: &str,
    rect: Rect,
    role: Role,
    name: &str,
    note: &str,
) {
    let id = region(ui, salt, rect, role, name);
    ui.ctx()
        .accesskit_node_builder(id, |node| node.set_description(note.to_owned()));
    draw_pane(ui, theme, rect, name, note);
}

/// What an element shows, from the shell's state.
fn look(element: Element, shell: &Shell) -> Look {
    let text = |t: &str| Look {
        text: Some(t.to_owned()),
        ..Look::default()
    };
    match element {
        Element::BrowserToggle => Look {
            on: shell.browser_visible(),
            value: Some(
                if shell.browser_visible() {
                    "shown"
                } else {
                    "hidden"
                }
                .to_owned(),
            ),
            ..Look::default()
        },
        Element::TapTempo => text("TAP"),
        // The transport section's values come from `control_bar`.
        Element::Quantization => text("1 Bar"),
        Element::RootNote => text("C"),
        Element::ScaleName => text("Major"),
        Element::KeyMap => text("KEY"),
        Element::MidiMap => text("MIDI"),
        Element::SampleRate => {
            let rate = shell.audio().status().sample_rate;
            let shown = rate.map_or_else(|| "—".to_owned(), sample_rate_text);
            Look {
                value: Some(
                    rate.map_or_else(|| "no audio device open".to_owned(), sample_rate_text),
                ),
                text: Some(shown),
                ..Look::default()
            }
        }
        Element::Cpu => {
            let status = shell.audio().status();
            match status.cpu_load.filter(|_| status.engine_on) {
                Some(load) => Look {
                    text: Some(format!("{:.0} %", load.ceil())),
                    value: Some(format!("{load:.1} percent")),
                    number: Some(f64::from(load)),
                    ..Look::default()
                },
                None => Look {
                    text: Some("OFF".to_owned()),
                    value: Some("audio engine off".to_owned()),
                    ..Look::default()
                },
            }
        }
        Element::Follow => {
            let view = shell.arrangement();
            Look {
                on: view.follow(),
                value: Some(
                    match (view.follow(), view.follow_paused()) {
                        (true, true) => "on, paused",
                        (true, false) => "on",
                        (false, _) => "off",
                    }
                    .to_owned(),
                ),
                ..Look::default()
            }
        }
        Element::SessionView => Look {
            on: shell.main_view() == MainView::Session,
            ..Look::default()
        },
        Element::ArrangementView => Look {
            on: shell.main_view() == MainView::Arrangement,
            ..Look::default()
        },
        Element::Message => match shell.message() {
            Some(message) => Look {
                text: Some(message.text.clone()),
                on: message.error,
                ..Look::default()
            },
            None => Look {
                text: Some(String::new()),
                ..Look::default()
            },
        },
        _ => Look::default(),
    }
}

/// A sample rate as the Control Bar shows it: `48 kHz`, `44.1 kHz`.
#[must_use]
pub fn sample_rate_text(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", f64::from(rate) / 1000.0)
    }
}

/// What clicking a live element runs.
fn click(element: Element, shell: &Shell) -> Option<Invocation> {
    match element {
        Element::BrowserToggle => Some(Invocation::new(ids::VIEW_TOGGLE_BROWSER, json!({}))),
        Element::Follow => Some(Invocation::new(
            bl_shell::arrangement_commands::FOLLOW,
            json!({}),
        )),
        Element::SampleRate => Some(Invocation::new(
            ids::APP_SETTINGS,
            json!({ "page": "audio" }),
        )),
        Element::SessionView if shell.main_view() != MainView::Session => Some(Invocation::new(
            ids::VIEW_TOGGLE_SESSION_ARRANGEMENT,
            json!({}),
        )),
        Element::ArrangementView if shell.main_view() != MainView::Arrangement => Some(
            Invocation::new(ids::VIEW_TOGGLE_SESSION_ARRANGEMENT, json!({})),
        ),
        Element::Message => shell.message().and_then(|m| m.action.clone()),
        // Play, Stop, and the Loop switch (WP-1.18).
        _ => control_bar::click(element, shell),
    }
}

/// Whether an element is one the person can operate now (not pending).
#[must_use]
pub fn is_live(element: Element) -> bool {
    let placement = crate::layout::placement(element);
    placement.pending.is_none() && !matches!(placement.kind, Kind::Indicator | Kind::Meter)
}

/// The menu bar to draw, where there's no native one.
#[derive(Clone, Copy, Debug)]
pub struct DrawnMenuBar<'a> {
    /// The menus.
    pub menus: &'a [bl_shell::menus::Menu],
    /// The keymap, for the keys each item shows.
    pub keymap: &'a crate::keymap::Keymap,
    /// Whose keys to show.
    pub platform: crate::keymap::Platform,
}
