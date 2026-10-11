//! A headless window for UI tests: an egui context with the theme, the
//! bundled typeface, and AccessKit on, driven by synthetic input.

#![allow(dead_code, reason = "each test file uses part of the harness")]

use bl_shell::{Effect, Shell};
use bl_ui::input::KeyDispatcher;
use bl_ui::keymap::{Keymap, Platform};
use bl_ui::layout::{REFERENCE, apply_zoom, follow_zoom};
use bl_ui::main_window::{MainWindowUi, UiEvent};
use bl_ui::settings_window::SettingsUi;
use bl_ui::theme::{Theme, fonts};
use egui::accesskit::{self, Role};
use egui::{Context, Event, FullOutput, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};

/// Which window a harness draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Main,
    Settings,
}

/// One accessible node: role, label, value, and whether it's disabled.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: accesskit::NodeId,
    pub role: Role,
    pub label: String,
    pub value: String,
    pub description: String,
    pub disabled: bool,
    /// Whether it's selected, for items that can be (WP-1.8b).
    pub selected: Option<bool>,
    pub numeric: Option<f64>,
    pub children: Vec<accesskit::NodeId>,
    pub bounds: Option<accesskit::Rect>,
}

pub struct Harness {
    pub ctx: Context,
    pub shell: Shell,
    pub theme: Theme,
    pub keymap: Keymap,
    pub dispatcher: KeyDispatcher,
    pub main: MainWindowUi,
    pub settings: SettingsUi,
    pub window: Window,
    pub time: f64,
    /// The window's size in window points: at a zoom other than 100%, egui
    /// lays out `size / zoom` layout points, as in the app.
    pub size: egui::Vec2,
    /// The interface zoom the window draws at, which follows the shell's as
    /// the app's windows do.
    pub zoom: f32,
    pub events: Vec<UiEvent>,
    /// Whether the window draws Live's title bar itself, as on macOS (D22).
    pub title_bar: bool,
    /// The transport the engine had published when the last frame began,
    /// which that frame drew (WP-1.18).
    pub engine: bl_shell::audio::EngineTransport,
}

impl Harness {
    pub fn new(window: Window) -> Self {
        let ctx = Context::default();
        ctx.set_fonts(fonts());
        ctx.enable_accesskit();
        let mut shell = Shell::headless();
        shell.start_audio();
        let theme = Theme::builtin(shell.theme());
        theme.apply(&ctx);
        let zoom = shell.zoom();
        apply_zoom(&ctx, zoom);
        let size = match window {
            Window::Main => REFERENCE,
            Window::Settings => {
                bl_ui::layout::SETTINGS_WINDOW - vec2(0.0, bl_ui::layout::TITLE_BAR)
            }
        };
        Self {
            ctx,
            shell,
            theme,
            keymap: Keymap::live12(),
            dispatcher: KeyDispatcher::new(Platform::current()),
            main: MainWindowUi::new(),
            settings: SettingsUi::new(),
            window,
            time: 0.0,
            size,
            zoom,
            events: Vec::new(),
            title_bar: false,
            engine: bl_shell::audio::EngineTransport::default(),
        }
    }

    /// A harness for a window that covers the system's title bar and draws
    /// Live's, as on macOS (D22): its content is the whole window, 1600 x
    /// 1000 points for the main window.
    pub fn with_title_bar(window: Window) -> Self {
        let mut harness = Self::new(window);
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
            Window::Main => {
                apply_zoom(&self.ctx, zoom);
                zoom
            }
            Window::Settings => follow_zoom(&self.ctx, self.zoom, zoom),
        };
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size / self.zoom)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        // As the app does: the keys go to a value being typed as to a text
        // field, and the frame draws what the engine last published.
        let text_focused = self.ctx.text_edit_focused() || self.main.is_typing();
        let invocations =
            self.dispatcher
                .take_bound_keys(&mut input, &self.keymap, &self.shell, text_focused);
        self.engine = self.shell.poll_engine();
        if self.window == Window::Settings {
            self.settings.prepare(&mut self.shell);
        }
        let shell = &self.shell;
        let theme = &self.theme;
        let main = &mut self.main;
        let settings = &mut self.settings;
        let window = self.window;
        let title_bar = self.title_bar;
        let mut ui_events = Vec::new();
        let mut output = self.ctx.run_ui(input, |ui| {
            ui_events = match window {
                Window::Main => main.show(ui, shell, theme, title_bar, None),
                Window::Settings => settings.show(ui, shell, theme, title_bar),
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
                UiEvent::FocusArea(area) => self.shell.set_focused_area(*area),
                UiEvent::ArrangementViewport { width, height } => {
                    self.shell.set_arrangement_viewport(*width, *height);
                }
                UiEvent::Gesture { phase, invocation } => {
                    let _ = self.shell.gesture(*phase, invocation.as_ref());
                }
                UiEvent::BeginGesture(label) => self.shell.begin_gesture(label),
                UiEvent::EndGesture => self.shell.end_gesture(),
            }
            self.events.push(event);
        }
        for effect in self.shell.take_effects() {
            match self.window {
                Window::Main => self.main.apply_effect(&self.ctx, &effect),
                Window::Settings => self.settings.apply_effect(&effect),
            }
            if matches!(effect, Effect::OpenSettings) {
                self.settings.refresh();
            }
        }
        output
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
                disabled: node.is_disabled(),
                selected: node.is_selected(),
                numeric: node.numeric_value(),
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
