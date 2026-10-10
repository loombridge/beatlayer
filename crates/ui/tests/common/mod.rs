//! A headless window for UI tests: an egui context with the theme, the
//! bundled typeface, and AccessKit on, driven by synthetic input.

#![allow(dead_code, reason = "each test file uses part of the harness")]

use bl_shell::{Effect, Shell};
use bl_ui::input::KeyDispatcher;
use bl_ui::keymap::{Keymap, Platform};
use bl_ui::layout::REFERENCE;
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
    pub size: egui::Vec2,
    pub events: Vec<UiEvent>,
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
            events: Vec::new(),
        }
    }

    /// Runs one frame with these input events: key presses go through the
    /// keymap first, as in the app. The invocations and UI events it caused
    /// are run on the shell, and their effects applied.
    pub fn frame(&mut self, events: Vec<Event>) -> FullOutput {
        self.time += 1.0 / 60.0;
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let text_focused = self.ctx.text_edit_focused();
        let invocations =
            self.dispatcher
                .take_bound_keys(&mut input, &self.keymap, &self.shell, text_focused);
        if self.window == Window::Settings {
            self.settings.prepare(&mut self.shell);
        }
        let shell = &self.shell;
        let theme = &self.theme;
        let main = &mut self.main;
        let settings = &mut self.settings;
        let window = self.window;
        let mut ui_events = Vec::new();
        let mut output = self.ctx.run_ui(input, |ui| {
            ui_events = match window {
                Window::Main => main.show(ui, shell, theme, None),
                Window::Settings => settings.show(ui, shell, theme),
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
