//! A headless window for UI tests: an egui context with the theme, the
//! bundled typeface, and AccessKit on, driven by synthetic input.

#![allow(dead_code, reason = "each test file uses part of the harness")]

use bl_audio_io::NullControl;
use bl_shell::testing;
use bl_shell::{Effect, Shell, WindowKind};
use bl_ui::input::KeyDispatcher;
use bl_ui::keymap::{Keymap, Platform};
use bl_ui::layout::{REFERENCE, apply_zoom, follow_zoom};
use bl_ui::main_window::{MainWindowUi, UiEvent};
use bl_ui::settings_window::SettingsUi;
use bl_ui::theme::{Theme, fonts};
use egui::accesskit::{self, Role};
use egui::{
    Color32, Context, Event, FullOutput, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2,
};
use serde_json::json;

/// Which window a harness draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Main,
    /// The second window (WP-6.29), on a shell where it is open.
    Second,
    Settings,
    /// A floating device's window (WP-2.5b).
    Device(bl_model::DeviceId),
    /// The assistant's panel in a window of its own (WP-2.23, part 3).
    Assistant,
}

/// One accessible node: role, label, value, and whether it's disabled.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: accesskit::NodeId,
    pub role: Role,
    pub label: String,
    pub value: String,
    pub description: String,
    /// The node's state, as a screen reader reads it (undo history R12).
    pub state: String,
    pub disabled: bool,
    /// Whether it's selected, for items that can be (WP-1.8b).
    pub selected: Option<bool>,
    /// Whether it's toggled, for switches (a switch's on state).
    pub toggled: Option<bool>,
    pub numeric: Option<f64>,
    /// The node a screen reader reads as the focus inside this one.
    pub active_descendant: Option<accesskit::NodeId>,
    pub children: Vec<accesskit::NodeId>,
    pub bounds: Option<accesskit::Rect>,
}

/// The other main window beside the harness's own (WP-6.29): its context and
/// what it draws, over the same shell.
pub struct Side {
    pub window: WindowKind,
    pub ctx: Context,
    pub ui: MainWindowUi,
    pub dispatcher: KeyDispatcher,
    pub size: egui::Vec2,
    pub time: f64,
    /// Whether it draws Live's title bar itself, as the harness' own window
    /// does.
    pub title_bar: bool,
}

pub struct Harness {
    pub ctx: Context,
    pub shell: Shell,
    pub theme: Theme,
    pub keymap: Keymap,
    pub dispatcher: KeyDispatcher,
    pub main: MainWindowUi,
    pub settings: SettingsUi,
    /// What a floating device's window draws (WP-2.5b).
    pub device_window: bl_ui::device_view::DeviceViewUi,
    /// What the floating assistant's window draws.
    pub assistant_window: bl_ui::assistant_panel::AssistantPanelUi,
    pub window: Window,
    /// The other of the two main windows, once [`Harness::open_other`] has
    /// made it.
    pub other: Option<Side>,
    pub time: f64,
    /// The window's size in window points: at a zoom other than 100%, egui
    /// lays out `size / zoom` layout points, as in the app.
    pub size: egui::Vec2,
    /// The interface zoom the window draws at, which follows the shell's as
    /// the app's windows do.
    pub zoom: f32,
    pub events: Vec<UiEvent>,
    /// The effects the shell asked the platform for, which the harness has
    /// no platform to carry out: the save dialog Export asks for, say.
    pub effects: Vec<Effect>,
    /// Whether the window draws Live's title bar itself, as on macOS (D22).
    pub title_bar: bool,
    /// The transport the engine had published when the last frame began,
    /// which that frame drew (WP-1.18).
    pub engine: bl_shell::audio::EngineTransport,
    /// The menu bar the main window draws itself, as on Linux, if any
    /// (WP-1.14).
    pub menu_bar: Option<Vec<bl_shell::menus::Menu>>,
    /// Files dropped into this window at the next frame, as a drag and
    /// drop from Finder or Explorer gives them.
    pub dropped: Vec<egui::DroppedFileHandle>,
    /// The blocks of a null stream paced by hand (T199), when the harness
    /// runs one: the engine advances only as the test asks or as its
    /// [`frame`](Self::frame)s run, never on the wall clock.
    control: Option<NullControl>,
}

/// Whether these events press a button or a key, which gives the window
/// they arrive in the focus.
fn takes_focus(events: &[Event]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            Event::PointerButton { pressed: true, .. } | Event::Key { pressed: true, .. }
        )
    })
}

/// The blocks one frame runs on a manually paced stream: a few, so a window
/// that draws what the engine published keeps up between the test's own
/// steps, as the display's frames would.
const BLOCKS_PER_FRAME: u64 = 2;

impl Harness {
    pub fn new(window: Window) -> Self {
        let mut shell = Shell::headless();
        shell.start_audio();
        Self::with_shell(window, shell)
    }

    /// A harness whose null stream is paced by hand (T199): the engine's
    /// blocks run only as the test's frames and [`advance`](Self::advance)
    /// ask, so a test that waits for an engine result never races the wall
    /// clock.
    pub fn manual(window: Window) -> Self {
        let (mut shell, control) = testing::rig();
        shell.start_audio();
        let mut harness = Self::with_shell(window, shell);
        harness.control = Some(control);
        harness
    }

    /// A harness whose shell journals the open set's edits in `dir`, as
    /// the app does at start-up (WP-6.62, the crash-recovery spec, R21),
    /// and which offers the sets a crash left.
    pub fn with_recovery(window: Window, dir: std::path::PathBuf) -> Self {
        let mut shell = Shell::headless();
        shell.start_audio();
        shell.use_recovery(dir).unwrap();
        Self::with_shell(window, shell)
    }

    fn with_shell(window: Window, mut shell: Shell) -> Self {
        if window == Window::Second {
            // The second window of a set opens on the other main view.
            shell.run("view.toggle_second_window", json!({})).unwrap();
        }
        let ctx = Context::default();
        ctx.set_fonts(fonts());
        ctx.enable_accesskit();
        let theme = Theme::of(&shell);
        theme.apply(&ctx);
        let zoom = shell.zoom();
        apply_zoom(&ctx, zoom);
        let size = match window {
            Window::Main => REFERENCE,
            Window::Second => bl_ui::layout::SECOND_WINDOW - vec2(0.0, bl_ui::layout::TITLE_BAR),
            Window::Settings => {
                bl_ui::layout::SETTINGS_WINDOW - vec2(0.0, bl_ui::layout::TITLE_BAR)
            }
            Window::Device(_) => vec2(400.0, 300.0),
            Window::Assistant => vec2(380.0, 600.0),
        };
        Self {
            ctx,
            shell,
            theme,
            keymap: Keymap::live12(),
            dispatcher: KeyDispatcher::new(Platform::current()),
            main: if window == Window::Second {
                MainWindowUi::second()
            } else {
                MainWindowUi::new()
            },
            other: None,
            settings: SettingsUi::new(),
            device_window: bl_ui::device_view::DeviceViewUi::new(),
            assistant_window: bl_ui::assistant_panel::AssistantPanelUi::default(),
            window,
            time: 0.0,
            size,
            zoom,
            events: Vec::new(),
            effects: Vec::new(),
            title_bar: false,
            engine: bl_shell::audio::EngineTransport::default(),
            menu_bar: None,
            dropped: Vec::new(),
            control: None,
        }
    }

    /// Runs `blocks` blocks of the manually paced null stream, if the harness
    /// has one: the engine advances as the test asks (T199).
    pub fn advance(&mut self, blocks: u64) {
        if let Some(control) = &self.control
            && self.shell.audio().is_running()
        {
            control.run_blocks(blocks);
        }
    }

    /// Runs frames, each advancing a manually paced engine, until `done`
    /// holds; panics with `what` after 2,000 frames (about 6 s of test time).
    pub fn wait(&mut self, what: &str, mut done: impl FnMut(&Harness) -> bool) {
        for _ in 0..2_000 {
            // The app's loop takes the background work's news every frame:
            // the browser's index, a decoded preview, a finished scan.
            self.shell.poll();
            self.frame(vec![]);
            if done(self) {
                return;
            }
        }
        panic!("waited for {what}");
    }

    /// A harness for a window that covers the system's title bar and draws
    /// Live's, as on macOS (D22): its content is the whole window, 1600 x
    /// 1000 points for the main window.
    pub fn with_title_bar(window: Window) -> Self {
        Self::titled(Self::new(window), window)
    }

    /// The same, with the null stream paced by hand (T199), for a test that
    /// waits for an engine result.
    pub fn manual_with_title_bar(window: Window) -> Self {
        Self::titled(Self::manual(window), window)
    }

    fn titled(mut harness: Self, _window: Window) -> Self {
        harness.size += vec2(0.0, bl_ui::layout::TITLE_BAR);
        harness.title_bar = true;
        harness
    }

    /// Runs one frame with these input events: key presses go through the
    /// keymap first, as in the app. The invocations and UI events it caused
    /// are run on the shell, and their effects applied.
    pub fn frame(&mut self, events: Vec<Event>) -> FullOutput {
        self.time += 1.0 / 60.0;
        // The zoom follows the shell's as in the app: the main window's at
        // once, the Settings window's once no mouse button is down in it.
        let zoom = self.shell.zoom();
        self.zoom = match self.window {
            Window::Main | Window::Device(_) | Window::Assistant => {
                apply_zoom(&self.ctx, zoom);
                zoom
            }
            // Spec: interface-zoom R11. The second window has a zoom of its
            // own.
            Window::Second => {
                let zoom = self.shell.zoom_of(WindowKind::Second);
                apply_zoom(&self.ctx, zoom);
                zoom
            }
            Window::Settings => follow_zoom(&self.ctx, self.zoom, zoom),
        };
        // A press in a window gives it the focus, as the OS does.
        if self.other.is_some() && takes_focus(&events) {
            self.shell.set_focused_window(self.kind());
        }
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size / self.zoom)),
            time: Some(self.time),
            events,
            dropped_files: std::mem::take(&mut self.dropped),
            ..Default::default()
        };
        // Spec: view-options R1. The middle button pans the area under the
        // pointer, so egui never sees it.
        bl_ui::middle::take(&mut input, &self.ctx);
        // As the app does: the keys go to a value being typed as to a text
        // field, and the frame draws what the engine last published.
        let text_focused = self.ctx.text_edit_focused() || self.main.is_typing();
        let invocations = if self.window == Window::Settings && self.settings.recording() {
            // The keys being recorded on the Shortcuts page are the
            // person's, not the app's (the custom shortcuts spec, R8).
            self.settings
                .take_recorded_key(&mut input, &self.shell)
                .into_iter()
                .collect()
        } else {
            self.dispatcher
                .take_bound_keys(&mut input, &self.keymap, &self.shell, text_focused)
        };
        self.advance(BLOCKS_PER_FRAME);
        self.engine = self.shell.poll_engine();
        if self.window == Window::Settings {
            self.settings.prepare(&mut self.shell);
        }
        let shell = &self.shell;
        let theme = &self.theme;
        let main = &mut self.main;
        let settings = &mut self.settings;
        let device_window = &mut self.device_window;
        let assistant_window = &mut self.assistant_window;
        let window = self.window;
        let title_bar = self.title_bar;
        let drawn = self
            .menu_bar
            .as_deref()
            .map(|menus| bl_ui::main_window::DrawnMenuBar {
                menus,
                keymap: &self.keymap,
                platform: Platform::Other,
            });
        let mut ui_events = Vec::new();
        let mut output = self.ctx.run_ui(input, |ui| {
            ui_events = match window {
                Window::Main | Window::Second => main.show(ui, shell, theme, title_bar, drawn),
                Window::Settings => settings.show(ui, shell, theme, title_bar),
                Window::Assistant => {
                    let mut events = Vec::new();
                    let rect = ui.max_rect();
                    assistant_window.show(ui, shell, theme, rect, false, &mut events);
                    events
                }
                Window::Device(device) => {
                    let mut events = Vec::new();
                    if let Some(floating) = shell
                        .floating_windows()
                        .windows
                        .into_iter()
                        .find(|w| w.device == device)
                    {
                        device_window.show_window(ui, shell, theme, &floating, 0.0, &mut events);
                    }
                    events
                }
            };
        });
        output.textures_delta.clear();
        for invocation in invocations {
            let _ = self.shell.invoke(&invocation.id, invocation.args.clone());
            self.events.push(UiEvent::Invoke(invocation));
        }
        for event in ui_events {
            match &event {
                UiEvent::Invoke(invocation) => {
                    let _ = self.shell.invoke(&invocation.id, invocation.args.clone());
                }
                UiEvent::FocusArea(area) => self.shell.set_focused_area_in(self.kind(), *area),
                UiEvent::FocusTrack(track) => self.shell.set_focused_track(*track),
                UiEvent::FocusValue(focused) => self.shell.set_value_focused(*focused),
                UiEvent::FocusControl(focused) => {
                    self.shell.set_envelope_control_focused(*focused);
                }
                UiEvent::StatusHint(hint) => self.shell.set_hint(hint.clone()),
                UiEvent::SearchRun(index) => self.shell.search_run_row(*index),
                UiEvent::ArrangementViewport { width, height } => {
                    self.shell.set_arrangement_viewport(*width, *height);
                }
                UiEvent::Gesture { phase, invocation } => {
                    let _ = self.shell.gesture(*phase, invocation.as_ref());
                }
                UiEvent::BeginGesture(label) => self.shell.begin_gesture(label),
                UiEvent::BeginNamedGesture(label) => self.shell.begin_gesture(label),
                UiEvent::EndGesture => self.shell.end_gesture(),
            }
            self.events.push(event);
        }
        self.apply_effects();
        self.share_hover();
        output
    }

    /// Tells the main window's Info View what the pointer is over in the
    /// other window (info-view R8), as the app does.
    fn share_hover(&mut self) {
        let Some(side) = &mut self.other else {
            return;
        };
        if self.window == Window::Main {
            self.main.set_other_hovered(side.ui.hovered());
        } else if side.window == WindowKind::Main {
            side.ui.set_other_hovered(self.main.hovered());
        }
    }

    /// The kind of main window this harness draws.
    pub fn kind(&self) -> WindowKind {
        match self.window {
            Window::Second => WindowKind::Second,
            _ => WindowKind::Main,
        }
    }

    /// Carries out the effects the shell asked for, in the window each is
    /// for.
    fn apply_effects(&mut self) {
        for effect in self.shell.take_effects() {
            self.effects.push(effect.clone());
            let target = match effect {
                Effect::FocusSecondArea(_) => WindowKind::Second,
                Effect::MoveFocus { .. } => self.shell.focused_window(),
                _ => WindowKind::Main,
            };
            if let Some(other) = &mut self.other
                && other.window == target
            {
                other.ui.apply_effect(&other.ctx, &effect);
            } else if matches!(self.window, Window::Main | Window::Second) && self.kind() == target
            {
                self.main.apply_effect(&self.ctx, &effect);
            } else if self.window == Window::Settings {
                self.settings.apply_effect(&effect);
            }
            if matches!(effect, Effect::OpenSettings) {
                self.settings.refresh();
            }
        }
    }

    /// Opens the other main window beside this one, on the same shell: the
    /// second window beside the main one, or the main one beside the second.
    pub fn open_other(&mut self) {
        let window = if self.kind() == WindowKind::Main {
            if !self.shell.second_window_open() {
                self.shell
                    .run("view.toggle_second_window", json!({}))
                    .unwrap();
            }
            WindowKind::Second
        } else {
            WindowKind::Main
        };
        let ctx = Context::default();
        ctx.set_fonts(fonts());
        ctx.enable_accesskit();
        self.theme.apply(&ctx);
        apply_zoom(&ctx, self.shell.zoom_of(window));
        let size = if window == WindowKind::Main {
            REFERENCE
        } else {
            bl_ui::layout::SECOND_WINDOW - vec2(0.0, bl_ui::layout::TITLE_BAR)
        };
        self.other = Some(Side {
            window,
            ctx,
            ui: if window == WindowKind::Main {
                MainWindowUi::new()
            } else {
                MainWindowUi::second()
            },
            dispatcher: KeyDispatcher::new(Platform::current()),
            size,
            time: 0.0,
            title_bar: self.title_bar,
        });
    }

    /// Runs one frame of the other main window with these input events;
    /// input gives it the keyboard focus, as the OS does.
    pub fn other_frame(&mut self, events: Vec<Event>) -> FullOutput {
        let side = self.other.as_mut().expect("open_other first");
        let window = side.window;
        if takes_focus(&events) {
            self.shell.set_focused_window(window);
        }
        side.time += 1.0 / 60.0;
        let zoom = self.shell.zoom_of(window);
        apply_zoom(&side.ctx, zoom);
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, side.size / zoom)),
            time: Some(side.time),
            events,
            ..Default::default()
        };
        bl_ui::middle::take(&mut input, &side.ctx);
        let text_focused = side.ctx.text_edit_focused() || side.ui.is_typing();
        let invocations =
            side.dispatcher
                .take_bound_keys(&mut input, &self.keymap, &self.shell, text_focused);
        let (shell, theme, title_bar) = (&self.shell, &self.theme, side.title_bar);
        let ui = &mut side.ui;
        let mut ui_events = Vec::new();
        let mut output = side.ctx.run_ui(input, |root| {
            ui_events = ui.show(root, shell, theme, title_bar, None);
        });
        output.textures_delta.clear();
        for invocation in invocations {
            let _ = self.shell.invoke(&invocation.id, invocation.args.clone());
            self.events.push(UiEvent::Invoke(invocation));
        }
        for event in ui_events {
            match &event {
                UiEvent::Invoke(invocation) => {
                    let _ = self.shell.invoke(&invocation.id, invocation.args.clone());
                }
                UiEvent::FocusArea(area) => self.shell.set_focused_area_in(window, *area),
                UiEvent::FocusTrack(track) => self.shell.set_focused_track(*track),
                UiEvent::FocusValue(focused) => self.shell.set_value_focused(*focused),
                UiEvent::FocusControl(focused) => {
                    self.shell.set_envelope_control_focused(*focused);
                }
                UiEvent::StatusHint(hint) => self.shell.set_hint(hint.clone()),
                UiEvent::SearchRun(index) => self.shell.search_run_row(*index),
                UiEvent::ArrangementViewport { width, height } => {
                    self.shell.set_arrangement_viewport(*width, *height);
                }
                UiEvent::Gesture { phase, invocation } => {
                    let _ = self.shell.gesture(*phase, invocation.as_ref());
                }
                UiEvent::BeginGesture(label) => self.shell.begin_gesture(label),
                UiEvent::BeginNamedGesture(label) => self.shell.begin_gesture(label),
                UiEvent::EndGesture => self.shell.end_gesture(),
            }
            self.events.push(event);
        }
        self.apply_effects();
        self.share_hover();
        output
    }

    /// A click in the other main window.
    pub fn other_click(&mut self, at: Pos2) -> FullOutput {
        self.other_frame(vec![Event::PointerMoved(at)]);
        self.other_frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        self.other_frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }])
    }

    /// A key press and release in the other main window.
    pub fn other_key(&mut self, key: egui::Key, modifiers: Modifiers) -> FullOutput {
        for pressed in [true, false] {
            self.other_frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            }]);
        }
        self.other_frame(vec![])
    }

    /// Runs frames of the other main window until egui has settled.
    pub fn other_settle(&mut self) -> FullOutput {
        self.other_frame(vec![]);
        self.other_frame(vec![])
    }

    /// The layout the other main window drew last.
    pub fn other_layout(&self) -> &bl_ui::layout::ShellLayout {
        self.other
            .as_ref()
            .and_then(|side| side.ui.drawn_layout())
            .expect("the other window drew a frame")
    }

    /// Runs frames until egui has settled (popups placed, focus applied).
    pub fn settle(&mut self) -> FullOutput {
        self.frame(vec![]);
        self.frame(vec![])
    }

    pub fn take_events(&mut self) -> Vec<UiEvent> {
        std::mem::take(&mut self.events)
    }

    /// Every accessible node in a frame's output.
    pub fn nodes(output: &FullOutput) -> Vec<Node> {
        let update = output
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("an AccessKit tree update");
        update
            .nodes
            .iter()
            .map(|(id, node)| Node {
                id: *id,
                role: node.role(),
                label: node.label().unwrap_or_default().to_owned(),
                value: node.value().unwrap_or_default().to_owned(),
                description: node.description().unwrap_or_default().to_owned(),
                state: node.state_description().unwrap_or_default().to_owned(),
                disabled: node.is_disabled(),
                selected: node.is_selected(),
                toggled: node
                    .toggled()
                    .map(|toggled| toggled == accesskit::Toggled::True),
                numeric: node.numeric_value(),
                active_descendant: node.active_descendant(),
                bounds: node.bounds(),
                children: node.children().to_vec(),
            })
            .collect()
    }

    /// Whether `node` hangs, at any depth, from `ancestor`.
    pub fn descends_from(
        nodes: &[Node],
        node: accesskit::NodeId,
        ancestor: accesskit::NodeId,
    ) -> bool {
        let parent = |id| {
            nodes
                .iter()
                .find(|n| n.children.contains(&id))
                .map(|n| n.id)
        };
        let mut at = node;
        while let Some(up) = parent(at) {
            if up == ancestor {
                return true;
            }
            at = up;
        }
        false
    }

    /// The node labelled `label`.
    pub fn node<'a>(nodes: &'a [Node], label: &str) -> &'a Node {
        nodes
            .iter()
            .find(|n| n.label == label)
            .unwrap_or_else(|| panic!("no node labelled {label:?}"))
    }

    /// A click at a point: press and release.
    pub fn click(&mut self, at: Pos2) -> FullOutput {
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        self.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }])
    }

    /// A drag with the middle mouse button held: Live pans the area under
    /// the pointer with it (view-options R1).
    pub fn middle_drag(&mut self, from: Pos2, to: Pos2) {
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Middle,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        for step in 1..=4 {
            let t = step as f32 / 4.0;
            self.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
        }
        self.frame(vec![Event::PointerButton {
            pos: to,
            button: PointerButton::Middle,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.settle();
    }

    /// A key press and release with modifiers.
    pub fn key(&mut self, key: egui::Key, modifiers: Modifiers) -> FullOutput {
        self.frame(vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }]);
        self.frame(vec![Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        }])
    }

    /// An AccessKit action on a widget, as a screen reader sends it.
    pub fn accesskit(&mut self, id: Id, action: accesskit::Action) -> FullOutput {
        self.frame(vec![Event::AccessKitActionRequest(
            accesskit::ActionRequest {
                action,
                target_tree: accesskit::TreeId::ROOT,
                target_node: id.accesskit_id(),
                data: None,
            },
        )])
    }
}

/// The platform's command modifier: Cmd on macOS, Ctrl elsewhere.
pub fn command() -> Modifiers {
    Modifiers::COMMAND
        | if cfg!(target_os = "macos") {
            Modifiers::MAC_CMD
        } else {
            Modifiers::CTRL
        }
}

/// Every rectangle a frame paints, and the colour it fills it with.
pub fn painted_rects(output: &FullOutput) -> Vec<(Rect, Color32)> {
    fn walk(shape: &egui::Shape, out: &mut Vec<(Rect, Color32)>) {
        match shape {
            egui::Shape::Rect(rect) => out.push((rect.rect, rect.fill)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}
