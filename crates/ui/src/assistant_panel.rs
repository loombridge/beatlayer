//! The built-in assistant's panel (WP-2.23, part 3; D24): the conversation,
//! a prompt field, the streamed reply, each tool call with its result, a
//! Stop button, and the provider and model in use.
//!
//! Live has no assistant, so the panel is Beatlayer's own design within
//! Live's layout. It docks as a full-height pane at the main window's right
//! edge, whose left edge the person drags to resize, or floats in a window
//! of its own; its header's Float and Dock button switches between them.
//! Every control runs a registered command ([`bl_shell::assistant`]), so a
//! click and a test reach the same implementation.

use bl_shell::assistant::{ASSISTANT_SET_PANEL_WIDTH, Row, WIDTH_RANGE};
use bl_shell::{Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align, Color32, CursorIcon, Id, Key, Layout, Rect, RichText, ScrollArea, Sense, Stroke, Ui,
    UiBuilder, pos2, vec2,
};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The header's height.
const HEADER: f32 = 24.0;
/// The prompt row's height.
const PROMPT: f32 = 28.0;
/// How far either side of the pane's left edge the drag handle reaches.
const HANDLE: f32 = 4.0;
/// How far an arrow key moves the pane's edge, in points.
const KEY_STEP: f32 = 16.0;
/// The most characters of a tool call's result the panel shows.
const RESULT_CHARS: usize = 160;

/// The panel's state between frames: what is typed into the prompt field,
/// and a drag on the pane's left edge.
#[derive(Debug, Default)]
pub struct AssistantPanelUi {
    prompt: String,
    /// The width the pane is being dragged to, until the drag ends and the
    /// width is set by `assistant.set_panel_width`.
    drag_width: Option<f32>,
    /// Whether the prompt field takes the keyboard focus on the next frame.
    focus_prompt: bool,
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

fn bounds(rect: Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.left().into(),
        y0: rect.top().into(),
        x1: rect.right().into(),
        y1: rect.bottom().into(),
    }
}

/// Adds a control to the main window's Tab order.
fn register(ui: &Ui, id: Id) {
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Assistant, None);
}

/// Names a widget for assistive technology.
fn name(ui: &Ui, id: Id, label: &str) {
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_label(label.to_owned());
    });
}

impl AssistantPanelUi {
    /// Asks for the keyboard focus to go to the prompt field.
    pub fn focus_prompt(&mut self) {
        self.focus_prompt = true;
    }

    /// The width the pane is being dragged to, if it is: the main window
    /// lays itself out with it while the drag goes on.
    #[must_use]
    pub fn drag_width(&self) -> Option<f32> {
        self.drag_width
    }

    /// Draws the panel in `rect`; `docked` for the pane at the main
    /// window's edge, which has the resize handle, and not for the floating
    /// window's.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        docked: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let region = Id::new("region-assistant");
        let _ = ui.interact(rect, region, Sense::hover());
        ui.ctx().accesskit_node_builder(region, |node| {
            node.set_role(Role::Group);
            node.set_label("Assistant".to_owned());
            node.set_bounds(bounds(rect));
        });
        ui.painter().rect_filled(rect, 0.0, theme.colors.panel.0);
        ui.scope_builder(
            UiBuilder::new().max_rect(rect).accessibility_parent(region),
            |ui| {
                let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER));
                let prompt =
                    Rect::from_min_max(pos2(rect.left(), rect.bottom() - PROMPT), rect.max);
                let log = Rect::from_min_max(header.left_bottom(), prompt.right_top());
                self.header(ui, shell, theme, header, events);
                self.conversation(ui, shell, theme, log);
                self.prompt_row(ui, shell, theme, prompt, events);
            },
        );
        if docked {
            self.edge(ui, theme, rect, events);
        }
        if shell.assistant_running() {
            // The streamed reply and the calls' results arrive between frames.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// The title, the provider and model in use, and the buttons that float
    /// or dock the panel and close it.
    fn header(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let state = shell.assistant_state();
        let model = if state.model.is_empty() {
            "no model set"
        } else {
            &state.model
        };
        let using = format!("{}, {model}", state.provider.label());
        let buttons = 2.0 * 22.0 + 8.0;
        let text = Rect::from_min_max(
            rect.min + vec2(8.0, 0.0),
            pos2(rect.right() - buttons, rect.bottom()),
        );
        ui.painter().text(
            text.left_center() - vec2(0.0, 5.0),
            egui::Align2::LEFT_CENTER,
            "Assistant",
            theme.font(),
            theme.colors.text.0,
        );
        // The provider and model in use, as a label of its own.
        let using_rect = Rect::from_min_max(pos2(text.left(), text.center().y), text.max);
        let response = ui.put(
            using_rect,
            egui::Label::new(
                RichText::new(&using)
                    .font(theme.small_font())
                    .color(theme.colors.text_dim.0),
            )
            .truncate(),
        );
        name(ui, response.id, &format!("Provider and model: {using}"));

        let button = |ui: &mut Ui, x: f32, glyph: &str, label: &str| {
            let at = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(20.0, 20.0));
            let response = ui
                .put(at, egui::Button::new(glyph).frame(false))
                .on_hover_text(label);
            name(ui, response.id, label);
            register(ui, response.id);
            response.clicked()
        };
        let float = if shell.assistant_panel_state().floating {
            "Dock Assistant"
        } else {
            "Float Assistant"
        };
        let glyph = if float.starts_with("Dock") {
            "\u{21e5}"
        } else {
            "\u{29c9}"
        };
        if button(ui, rect.right() - 2.0 * 22.0 - 4.0, glyph, float) {
            invoke(
                events,
                bl_shell::assistant::ASSISTANT_TOGGLE_FLOATING,
                json!({}),
            );
        }
        if button(ui, rect.right() - 22.0 - 2.0, "\u{00d7}", "Close Assistant") {
            invoke(
                events,
                bl_shell::assistant::VIEW_TOGGLE_ASSISTANT,
                json!({}),
            );
        }
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
    }

    /// The conversation, newest at the bottom.
    fn conversation(&self, ui: &mut Ui, shell: &Shell, theme: &Theme, rect: Rect) {
        let rows = shell.assistant_rows();
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 4.0))));
        let id = child.id().with("conversation");
        ScrollArea::vertical()
            .id_salt(id)
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                ui.set_width(ui.available_width());
                if rows.is_empty() {
                    ui.label(
                        RichText::new(
                            "Ask for something, and the assistant will do it in your set.",
                        )
                        .font(theme.small_font())
                        .color(theme.colors.text_dim.0),
                    );
                }
                for row in &rows {
                    draw_row(ui, theme, row);
                }
            });
    }

    /// The prompt field with Send, which becomes Stop while a turn runs.
    fn prompt_row(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let running = shell.assistant_running();
        let button = 56.0;
        let field = Rect::from_min_max(
            rect.min + vec2(8.0, 3.0),
            pos2(rect.right() - button - 12.0, rect.bottom() - 3.0),
        );
        let edit = egui::TextEdit::singleline(&mut self.prompt)
            .id_salt("assistant-prompt")
            .hint_text("Message the assistant")
            .font(theme.small_font())
            .interactive(!running);
        let response = ui.put(field, edit);
        name(ui, response.id, "Assistant Message");
        // A field that can't be typed into while the assistant works isn't
        // a stop on the Tab order.
        if !running {
            register(ui, response.id);
            if std::mem::take(&mut self.focus_prompt) {
                response.request_focus();
            }
        }
        let at = Rect::from_min_max(
            pos2(field.right() + 4.0, field.top()),
            pos2(rect.right() - 8.0, field.bottom()),
        );
        if running {
            let stop = ui.put(at, egui::Button::new("Stop"));
            name(ui, stop.id, "Stop Assistant");
            register(ui, stop.id);
            if stop.clicked() {
                invoke(events, bl_shell::assistant::ASSISTANT_STOP, json!({}));
            }
            return;
        }
        let entered = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let send = ui.put(at, egui::Button::new("Send"));
        name(ui, send.id, "Send Message");
        register(ui, send.id);
        if (send.clicked() || entered) && !self.prompt.trim().is_empty() {
            invoke(
                events,
                bl_shell::assistant::ASSISTANT_SEND,
                json!({ "text": std::mem::take(&mut self.prompt) }),
            );
            response.request_focus();
        }
    }

    /// The handle on the pane's left edge: dragging it sets the width.
    fn edge(&mut self, ui: &mut Ui, theme: &Theme, rect: Rect, events: &mut Vec<UiEvent>) {
        let handle = Rect::from_min_max(
            pos2(rect.left() - HANDLE, rect.top()),
            pos2(rect.left() + HANDLE, rect.bottom()),
        );
        let id = Id::new("assistant-edge");
        let response = ui
            .interact(handle, id, Sense::drag())
            .on_hover_cursor(CursorIcon::ResizeHorizontal);
        register(ui, id);
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Splitter);
            node.set_label("Assistant Width".to_owned());
            node.set_numeric_value(f64::from(rect.width()));
            node.set_min_numeric_value(f64::from(*WIDTH_RANGE.start()));
            node.set_max_numeric_value(f64::from(*WIDTH_RANGE.end()));
        });
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let width = (rect.right() - pointer.x).clamp(*WIDTH_RANGE.start(), *WIDTH_RANGE.end());
            self.drag_width = Some(width);
            ui.ctx().request_repaint();
        }
        if response.drag_stopped()
            && let Some(width) = self.drag_width.take()
        {
            invoke(events, ASSISTANT_SET_PANEL_WIDTH, json!({ "width": width }));
        }
        // The keyboard moves the focused edge: Left widens, Right narrows.
        if response.has_focus() {
            let step = ui.input(|i| {
                i.key_pressed(Key::ArrowLeft) as i8 - i.key_pressed(Key::ArrowRight) as i8
            });
            if step != 0 {
                let width = rect.width() + f32::from(step) * KEY_STEP;
                invoke(events, ASSISTANT_SET_PANEL_WIDTH, json!({ "width": width }));
            }
        }
        if response.hovered() || response.dragged() {
            ui.painter().line_segment(
                [rect.left_top(), rect.left_bottom()],
                Stroke::new(2.0, theme.colors.focus.0),
            );
        }
    }
}

/// Draws one row of the conversation.
fn draw_row(ui: &mut Ui, theme: &Theme, row: &Row) {
    let dim = theme.colors.text_dim.0;
    let text = theme.colors.text.0;
    let small = theme.small_font();
    ui.add_space(4.0);
    match row {
        Row::You(said) => {
            ui.label(RichText::new("You").font(small.clone()).color(dim));
            ui.label(RichText::new(said).font(theme.font()).color(text));
        }
        Row::Assistant(said) => {
            ui.label(RichText::new("Assistant").font(small.clone()).color(dim));
            ui.label(RichText::new(said).font(theme.font()).color(text));
        }
        Row::Call {
            name,
            command,
            arguments,
            result,
            ..
        } => {
            let called = command.as_deref().unwrap_or(name);
            let (state, colour) = match result {
                None => ("running".to_owned(), dim),
                Some(Ok(value)) => (format!("done: {}", shorten(&value.to_string())), text),
                Some(Err(why)) => (
                    format!("didn't run: {}", shorten(why)),
                    Color32::from_rgb(0xd0, 0x60, 0x50),
                ),
            };
            ui.label(
                RichText::new(called)
                    .font(small.clone())
                    .color(text)
                    .strong(),
            );
            ui.label(
                RichText::new(shorten(&arguments.to_string()))
                    .font(small.clone())
                    .color(dim),
            );
            ui.label(RichText::new(state).font(small).color(colour));
        }
        Row::Notice(note) => {
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.label(RichText::new(note).font(small).color(dim).italics());
            });
        }
    }
}

/// `text` cut to what the panel shows of a long argument or result.
fn shorten(text: &str) -> String {
    if text.chars().count() <= RESULT_CHARS {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(RESULT_CHARS).collect();
    cut.push('\u{2026}');
    cut
}
