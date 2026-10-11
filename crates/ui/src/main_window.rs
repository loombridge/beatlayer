//! The main window: the Control Bar, the browser, the main view, the detail
//! area, and the Status Bar, placed by the layout layer (L-WIN, L-CTRL,
//! L-STAT). The browser fills its column (WP-1.10), the main view shows the
//! Arrangement View (WP-1.8a) with its mixer, or the Session View's columns
//! and mixer (WP-1.9); views that later packages build are placeholders
//! that say so.

use std::time::Duration;

use bl_shell::{Area, Effect, GesturePhase, Invocation, MainView, Shell, commands as ids};
use egui::accesskit::Role;
use egui::{
    Align2, CentralPanel, Context, Frame, Id, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder,
};
use serde_json::json;

use crate::arrangement::ArrangementUi;
use crate::browser::BrowserUi;
use crate::control_bar::{self, TransportFields};
use crate::layout::{ELEMENTS, Element, Kind, ShellLayout, Strip, title_bar_height};
use crate::mixer::MixerUi;
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
    /// A move to the next or previous control, or to a neighbour, done at
    /// the start of the next frame from the order the last frame drew
    /// (WP-1.14; the accessibility spec, R3 and R4).
    focus_move: Option<FocusMove>,
    /// The controls the last frame drew that the keyboard reaches.
    order: crate::focus::Order,
    /// A focus move waiting for its control to be drawn, with the frames
    /// left to wait.
    pending: Option<(Pending, u8)>,
    /// The Arrangement View's caches and gestures (WP-1.8a).
    arrangement: ArrangementUi,
    /// The browser's scrolling and search field (WP-1.10).
    browser: BrowserUi,
    /// The Control Bar's transport fields: a value being typed, a drag
    /// (WP-1.18).
    transport: TransportFields,
    /// The mixers (WP-1.9).
    mixer: MixerUi,
    /// The keymap the Mixer Config Menu shows its items' keys from, read
    /// when it first opens.
    keymap: Option<crate::keymap::Keymap>,
    /// Whether a value was being typed into a track header's volume field
    /// when the last frame ended.
    mixer_typing: bool,
}

/// A move of the keyboard focus asked for by `focus.next`,
/// `focus.previous`, or a neighbour command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FocusMove {
    backwards: bool,
    neighbor: bool,
}

/// A focus move to a control the last frame didn't draw.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pending {
    /// A neighbour on a track brought into view.
    Control(crate::focus::TrackControl),
    /// The first (or last) of a track's controls, Tab from the view.
    Enter {
        place: crate::mixer::Place,
        track: bl_model::TrackId,
        backwards: bool,
    },
}

/// How many frames a focus move waits for its control to be drawn.
const PENDING_FRAMES: u8 = 8;

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
            focus_move: None,
            order: crate::focus::Order::default(),
            pending: None,
            arrangement: ArrangementUi::new(),
            browser: BrowserUi::new(),
            transport: TransportFields::new(),
            mixer: MixerUi::new(),
            keymap: None,
            mixer_typing: false,
        }
    }

    /// The mixers, to read what they drew.
    #[must_use]
    pub fn mixer(&self) -> &MixerUi {
        &self.mixer
    }

    /// The browser, to read what it drew.
    #[must_use]
    pub fn browser(&self) -> &BrowserUi {
        &self.browser
    }

    /// Whether a value is being typed into a Control Bar field: while it
    /// is, key presses go to the field rather than to shortcuts, as they do
    /// to a text field (the control bar transport spec, R10).
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.transport.is_typing() || self.mixer_typing
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
            Effect::MoveFocus {
                backwards,
                neighbor,
            } => {
                self.focus_move = Some(FocusMove {
                    backwards: *backwards,
                    neighbor: *neighbor,
                });
            }
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
        let mut events = Vec::new();
        // Spec: accessibility R3, R4, R7. The keyboard's moves go by the
        // order the last frame drew.
        self.order = crate::focus::take(ctx);
        crate::focus::set_outlined(ctx, shell.outline_view_in_focus());
        if let Some(step) = self.focus_move.take() {
            self.move_focus(ctx, shell, step, &mut events);
        }
        self.resolve_pending(ctx, &mut events);
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
                self.browser.show(ui, shell, theme, browser, &mut events);
            }
            self.main_view(ui, shell, theme, layout.main_view, &mut events);
            // Files dragged from the browser, or from Finder or Explorer,
            // land in the Arrangement (WP-1.10).
            self.browser
                .drop_overlay(ui, shell, theme, layout.main_view, &mut events);
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
            outline_focused_area(ui, shell, theme, &layout);
        });
        self.mixer_typing = crate::mixer::is_typing(ctx);
        let metering = shell
            .meters()
            .iter()
            .any(|m| m.peak.iter().chain(&m.rms).any(|level| *level > 0.0));
        if shell.playing() || metering {
            // The playhead and the Arrangement Position fields move with
            // the engine every frame (control-bar-transport R14), and the
            // meters until they're silent (mixer-view R10).
            ctx.request_repaint();
        } else if shell.audio().engine_on() {
            // The CPU meter updates a few times a second.
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        events
    }

    /// Carries out a focus move from the last frame's order (the
    /// accessibility spec, R3 and R4).
    fn move_focus(
        &mut self,
        ctx: &Context,
        shell: &Shell,
        step: FocusMove,
        events: &mut Vec<UiEvent>,
    ) {
        use crate::focus::Neighbor;
        let focused = ctx.memory(egui::Memory::focused);
        if step.neighbor {
            let Some(from) = focused else {
                return;
            };
            let tracks = track_order(shell);
            match self.order.neighbor(from, &tracks, step.backwards) {
                Some(Neighbor::Focus(id)) => focus_on(ctx, id, events),
                Some(Neighbor::Reveal(target)) => {
                    self.reveal(target.place, target.track, events);
                    self.pending = Some((Pending::Control(target), PENDING_FRAMES));
                }
                None => {}
            }
            return;
        }
        let in_view = focused == Some(main_view_id())
            || (focused.is_none() && shell.focused_area() == Area::MainView);
        if in_view {
            // From the view, Tab goes into the selected track's controls (or
            // the first track's): the headers' in the Arrangement, the
            // mixer's in the Session View.
            let place = match shell.main_view() {
                MainView::Arrangement => crate::mixer::Place::Header,
                MainView::Session => {
                    crate::mixer::Place::Mixer(bl_shell::mixer_view::MixerOf::Session)
                }
            };
            let tracks = track_order(shell);
            let Some(track) = shell
                .arrangement()
                .selected_tracks()
                .iter()
                .find(|t| tracks.contains(t))
                .or(tracks.first())
                .copied()
            else {
                return;
            };
            match self.order.enter_track(place, track, step.backwards) {
                Some(id) => focus_on(ctx, id, events),
                None if !self.order.drew_track(place, track) => {
                    self.reveal(place, track, events);
                    self.pending = Some((
                        Pending::Enter {
                            place,
                            track,
                            backwards: step.backwards,
                        },
                        PENDING_FRAMES,
                    ));
                }
                None => {}
            }
            return;
        }
        if let Some(id) = self.order.tab(
            focused,
            shell.focused_area(),
            step.backwards,
            shell.wrap_navigation(),
        ) {
            focus_on(ctx, id, events);
        }
    }

    /// Brings a track into view in the headers or a mixer, for a focus
    /// move to one of its controls.
    fn reveal(
        &mut self,
        place: crate::mixer::Place,
        track: bl_model::TrackId,
        events: &mut Vec<UiEvent>,
    ) {
        match place {
            crate::mixer::Place::Header => events.push(UiEvent::Invoke(Invocation::new(
                bl_shell::arrangement_commands::SHOW_TRACK,
                json!({ "track": track }),
            ))),
            crate::mixer::Place::Mixer(of) => self.mixer.reveal(of, track),
        }
    }

    /// Focuses a control a focus move waits for, once it's drawn.
    fn resolve_pending(&mut self, ctx: &Context, events: &mut Vec<UiEvent>) {
        let Some((pending, frames)) = self.pending.take() else {
            return;
        };
        let found = match pending {
            Pending::Control(control) => self.order.control(control),
            Pending::Enter {
                place,
                track,
                backwards,
            } => self.order.enter_track(place, track, backwards),
        };
        match found {
            Some(id) => focus_on(ctx, id, events),
            None if frames > 1 => self.pending = Some((pending, frames - 1)),
            None => {}
        }
    }

    /// The node a screen reader reads as the Arrangement View's focus while
    /// the view itself has the keyboard: the selected clip, or the selected
    /// track's title (the accessibility spec, R2).
    fn active_descendant(&self, ctx: &Context, shell: &Shell) -> Option<Id> {
        if shell.main_view() != MainView::Arrangement {
            return None;
        }
        let view = shell.arrangement();
        if let Some(clip) = self.arrangement.selected_clip_node(view) {
            return Some(clip);
        }
        let drawn = crate::focus::drawn(ctx);
        view.selected_tracks().iter().find_map(|track| {
            drawn.control(crate::focus::TrackControl {
                place: crate::mixer::Place::Header,
                track: *track,
                slot: crate::focus::Slot::Title,
            })
        })
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
            let mut look = self
                .transport
                .look(shell, *el)
                .unwrap_or_else(|| look(*el, shell));
            if *el == Element::Message
                && let Some(readout) = crate::mixer::take_readout(ui.ctx())
            {
                // Spec: mixer-view R6. While a fader is dragged, the Status
                // Bar shows its volume.
                look = Look {
                    text: Some(readout),
                    ..Look::default()
                };
            }
            let response = element(ui, theme, placement, *rect, &look);
            if response.sense.is_focusable() {
                let area = match strip {
                    Strip::ControlBar => Area::ControlBar,
                    Strip::StatusBar => Area::StatusBar,
                };
                crate::focus::register(ui.ctx(), response.id, area, None);
            }
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
            if *el == Element::MixerMenu {
                self.mixer_menu(&response, shell, events);
            }
        }
    }

    /// The Mixer Config Menu (L-STAT-008): the Mixer Controls choices, as
    /// View ▸ Mixer Controls has them (the mixer view spec, R12).
    fn mixer_menu(&mut self, response: &egui::Response, shell: &Shell, events: &mut Vec<UiEvent>) {
        let keymap = self
            .keymap
            .get_or_insert_with(crate::keymap::Keymap::live12);
        let platform = crate::keymap::Platform::current();
        let menus = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current());
        let Some(controls) = menus
            .iter()
            .flat_map(bl_shell::menus::Menu::walk)
            .find(|item| item.row == "M-VIEW-032")
        else {
            return;
        };
        egui::Popup::menu(response).show(|ui| {
            for item in bl_shell::menus::shown(&controls.items) {
                crate::menus::item(ui, item, shell, keymap, platform, events);
            }
        });
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
                "The tracks' columns and their mixer. The clip slots and scenes arrive in \
                 milestone M3.",
            ),
        };
        let response = ui.interact(rect, main_view_id(), Sense::click());
        crate::focus::register(ui.ctx(), response.id, Area::MainView, None);
        let pressed_here = ui.input(|i| {
            i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|p| rect.contains(p))
        });
        if self.focus_request == Some(FocusRequest::MainView) || pressed_here {
            // A click in the view gives it the keyboard (I-FOCUS-001).
            response.request_focus();
            self.focus_request = None;
        }
        match shell.main_view() {
            MainView::Arrangement => {
                self.arrangement.show(ui, shell, theme, rect, events);
                if let Some(mixer) = self.arrangement.mixer_rect() {
                    self.mixer.show_mixer(
                        ui,
                        shell,
                        theme,
                        mixer,
                        bl_shell::mixer_view::MixerOf::Arrangement,
                        events,
                    );
                }
            }
            MainView::Session => self.mixer.show_session(ui, shell, theme, rect, events),
        }
        // Spec: accessibility R2. With the view focused, the clip or track
        // the keys select is its active descendant, so a screen reader reads
        // it as the selection moves.
        let active = self.active_descendant(ui.ctx(), shell);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Panel, true, name));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Pane);
            node.set_description(note.to_owned());
            if let Some(id) = active {
                node.set_active_descendant(id.accesskit_id());
            }
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

/// Moves the keyboard focus to a control: a part of the browser through
/// `focus.browser`, so the shell knows which part's keys apply, and any
/// other control directly.
fn focus_on(ctx: &Context, id: Id, events: &mut Vec<UiEvent>) {
    match crate::browser::part_of(id) {
        Some(part) => events.push(UiEvent::Invoke(Invocation::new(
            bl_shell::browser_commands::FOCUS_BROWSER,
            json!({ "part": part }),
        ))),
        None => ctx.memory_mut(|m| m.request_focus(id)),
    }
}

/// The tracks in the order the neighbour keys move through them: the
/// mixer's columns (the Arrangement's track order), then the returns and
/// Main.
fn track_order(shell: &Shell) -> Vec<bl_model::TrackId> {
    let doc = shell.editor().document();
    let mut tracks = shell.mixer_columns();
    tracks.extend(doc.returns());
    tracks.push(doc.main_id());
    tracks
}

/// Draws a border round the area with the keyboard focus while Outline View
/// in Focus is on (the accessibility spec, R7; I-FOCUS-009). It's off by
/// default, as Live's is, and then nothing shows where the focus is.
fn outline_focused_area(ui: &Ui, shell: &Shell, theme: &Theme, layout: &ShellLayout) {
    if !shell.outline_view_in_focus() || shell.focused_window() != bl_shell::WindowKind::Main {
        return;
    }
    let rect = match shell.focused_area() {
        Area::ControlBar => Some(layout.control_bar),
        Area::Browser => layout.browser,
        Area::MainView => Some(layout.main_view),
        Area::Detail => Some(layout.detail),
        Area::StatusBar => Some(layout.status_bar),
    };
    if let Some(rect) = rect {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            0.0,
            Stroke::new(1.0, theme.colors.focus.0),
            StrokeKind::Inside,
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
        Element::MixerToggle => {
            let shown = shell.mixer_view().of(shell.front_mixer()).shown;
            Look {
                on: shown,
                value: Some(if shown { "shown" } else { "hidden" }.to_owned()),
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
        // Spec: mixer-view R11.
        Element::MixerToggle => Some(Invocation::new(
            bl_shell::mixer_view_commands::TOGGLE,
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
