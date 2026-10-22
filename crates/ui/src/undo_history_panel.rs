//! The Undo History panel (WP-6.28): the set's steps, newest first, with
//! the ones that can be redone greyed, and the set's start at the bottom.
//!
//! Clicking a row, or moving to it with the arrow keys and pressing Enter,
//! returns the set to that point: the steps above it are undone, and the
//! greyed steps down to it are redone (undo history R6). The panel takes
//! the keyboard focus when it opens, with the current row highlighted, and
//! gives it back to the main view when it closes (R8).

use bl_shell::commands::VIEW_TOGGLE_UNDO_HISTORY;
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::Role;
use egui::{Align2, Color32, Id, Key, Rect, ScrollArea, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The header's height (L-LRN-002).
const HEADER: f32 = 21.0;
/// A row's height (undo history R4).
const ROW: f32 = 18.0;
/// The width the agent's mark takes before a row's label.
const MARK: f32 = 12.0;
/// The margin inside the panel.
const MARGIN: f32 = 6.0;

/// A row of the panel: a step of the history, or the set's start.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    /// The row's label: the step's, or the set's start's.
    label: String,
    /// Who made the step, or nothing for the set's start.
    maker: Option<bl_model::Maker>,
    /// Whether the step has been undone and can be redone.
    undone: bool,
    /// The step's place in the history, counted up from the set's start:
    /// what `edit.go_to_step` takes.
    index: usize,
}

/// The panel's state between frames: the row the keyboard has moved to, and
/// whether the list takes the keyboard focus.
#[derive(Debug, Default)]
pub struct UndoHistoryPanelUi {
    /// The row the keyboard has moved to, as its place in the list.
    highlight: Option<usize>,
    /// Whether the list takes the keyboard focus.
    focus: bool,
    /// Whether the panel showed last time, so opening and closing it can
    /// move the keyboard focus.
    shown: bool,
    /// The history the list was last drawn for, so a change scrolls the
    /// current row into view.
    drawn: Option<(usize, usize)>,
}

impl UndoHistoryPanelUi {
    /// The panel's rows, from the top: the steps that can be redone first,
    /// the newest of them first, then the steps that can be undone, and the
    /// set's start at the bottom.
    fn rows(shell: &Shell) -> Vec<Row> {
        let mut rows: Vec<Row> = shell
            .undo_history_rows()
            .iter()
            .map(|step| Row {
                label: step.label.to_string(),
                maker: Some(step.maker.clone()),
                undone: step.undone,
                index: step.index,
            })
            .collect();
        rows.push(Row {
            label: shell.set_start_label().to_owned(),
            maker: None,
            undone: false,
            index: 0,
        });
        rows
    }

    /// The current row's place in the list: the newest step that can be
    /// undone, or the set's start when there is none.
    fn current(rows: &[Row]) -> usize {
        rows.iter()
            .position(|row| !row.undone && row.index > 0)
            .unwrap_or(rows.len().saturating_sub(1))
    }

    /// Draws the panel in `rect`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let rows = Self::rows(shell);
        let current = Self::current(&rows);
        if !self.shown {
            // Spec: undo history R8. Opening the panel gives it the
            // keyboard focus with the current row highlighted.
            self.shown = true;
            self.focus = true;
            self.highlight = Some(current);
            events.push(UiEvent::FocusArea(Area::UndoHistory));
        }

        let region = Id::new("region-undo-history");
        let _ = ui.interact(rect, region, Sense::hover());
        ui.ctx().accesskit_node_builder(region, |node| {
            node.set_role(Role::Group);
            node.set_label("Undo History".to_owned());
            node.set_bounds(bounds(rect));
        });
        ui.painter().rect_filled(rect, 0.0, theme.colors.panel.0);
        ui.scope_builder(
            UiBuilder::new().max_rect(rect).accessibility_parent(region),
            |ui| {
                let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER));
                let list = Rect::from_min_max(
                    pos2(rect.left(), header.bottom()),
                    pos2(rect.right(), rect.bottom()),
                );
                self.header(ui, theme, header, events);
                self.list(ui, theme, list, &rows, current, events);
            },
        );
    }

    /// The panel is hidden: it gives the keyboard focus back to the main
    /// view when it had it.
    pub fn hide(&mut self, events: &mut Vec<UiEvent>) {
        if self.shown && self.focus {
            events.push(UiEvent::FocusArea(Area::MainView));
        }
        self.shown = false;
        self.focus = false;
        self.highlight = None;
    }

    /// The title and the close button at the left of the header, as the
    /// Learn View's header has them (L-LRN-002).
    fn header(&mut self, ui: &mut Ui, theme: &Theme, rect: Rect, events: &mut Vec<UiEvent>) {
        let at = Rect::from_min_size(pos2(rect.left() + 4.0, rect.top() + 1.0), vec2(18.0, 18.0));
        let response = ui
            .put(at, egui::Button::new("\u{00d7}").frame(false))
            .on_hover_text("Close Undo History");
        name(ui, response.id, "Close Undo History");
        register(ui, response.id);
        if response.clicked() {
            invoke(events, VIEW_TOGGLE_UNDO_HISTORY, json!({}));
        }
        let text = Rect::from_min_max(pos2(at.right() + 4.0, rect.top()), rect.max);
        ui.painter().text(
            text.left_center() - vec2(0.0, 5.0),
            Align2::LEFT_CENTER,
            "Undo History",
            theme.font(),
            theme.colors.text.0,
        );
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
    }

    /// The list of rows, with the current one selected and the greyed ones
    /// dimmed.
    fn list(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        rect: Rect,
        rows: &[Row],
        current: usize,
        events: &mut Vec<UiEvent>,
    ) {
        let list = Id::new("undo-history-list");
        let response = ui.interact(rect, list, Sense::click());
        if self.focus {
            response.request_focus();
        }
        if response.has_focus() {
            self.keys(ui, rows, current, events);
        }
        // The list box in the accessibility tree, with the rows in it.
        ui.ctx().accesskit_node_builder(list, |node| {
            node.set_role(Role::ListBox);
            node.set_label("Undo History Steps".to_owned());
            node.set_bounds(bounds(rect));
            if let Some(highlight) = self.highlight
                && let Some(row) = rows.get(highlight)
            {
                node.set_active_descendant(
                    Id::new("undo-history-row").with(row.index).accesskit_id(),
                );
            }
        });

        // The list scrolls to keep the current row in view when the
        // history changes (R5).
        let history = (rows.len(), current);
        let scroll = self.drawn != Some(history);
        self.drawn = Some(history);
        let height = rect.height();
        let offset = if scroll {
            let top = current as f32 * ROW;
            let mut offset = 0.0;
            if top < offset {
                offset = top;
            } else if top + ROW > offset + height {
                offset = top + ROW - height;
            }
            offset.max(0.0)
        } else {
            0.0
        };
        let mut area = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(MARGIN, 2.0)))
                .id_salt("undo-history-scroll")
                .accessibility_parent(list),
        );
        area.spacing_mut().item_spacing.y = 0.0;
        let mut scroll_area = ScrollArea::vertical().id_salt("undo-history-rows");
        if scroll {
            scroll_area = scroll_area.vertical_scroll_offset(offset);
        }
        scroll_area.show_rows(&mut area, ROW, rows.len(), |ui, range| {
            let first = range.start;
            // The rows' place is taken once: putting a label in the row
            // moves the area the rest are laid out in.
            let (left, top, width) = (
                ui.max_rect().left(),
                ui.max_rect().top(),
                ui.max_rect().width(),
            );
            for index in range {
                let row = &rows[index];
                let at = Rect::from_min_size(
                    pos2(left, top + (index - first) as f32 * ROW),
                    vec2(width, ROW),
                );
                let clicked = draw_row(
                    ui,
                    theme,
                    at,
                    row,
                    index == current,
                    self.highlight == Some(index),
                );
                if clicked {
                    self.focus = true;
                    self.highlight = Some(index);
                    self.jump(events, row);
                }
            }
        });
    }

    /// The keyboard: Up and Down move the highlight, Enter jumps to it, and
    /// Esc gives the focus back to the main view (R8).
    fn keys(&mut self, ui: &mut Ui, rows: &[Row], current: usize, events: &mut Vec<UiEvent>) {
        let step =
            ui.input(|i| i.key_pressed(Key::ArrowDown) as i8 - i.key_pressed(Key::ArrowUp) as i8);
        if step != 0 {
            let last = rows.len().saturating_sub(1);
            let from = self.highlight.unwrap_or(current);
            let next = if step > 0 {
                (from + 1).min(last)
            } else {
                from.saturating_sub(1)
            };
            self.highlight = Some(next);
        }
        if ui.input(|i| i.key_pressed(Key::Enter))
            && let Some(highlight) = self.highlight
            && let Some(row) = rows.get(highlight)
        {
            self.jump(events, row);
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            self.focus = false;
            events.push(UiEvent::FocusArea(Area::MainView));
        }
    }

    /// Returns the set to the point the row names.
    fn jump(&mut self, events: &mut Vec<UiEvent>, row: &Row) {
        invoke(
            events,
            bl_model::registry::GO_TO_STEP,
            json!({ "index": row.index }),
        );
    }
}

/// Draws one row: the agent's mark, the label, and the row's state. It
/// says whether the row was clicked.
fn draw_row(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    row: &Row,
    current: bool,
    highlighted: bool,
) -> bool {
    let id = Id::new("undo-history-row").with(row.index);
    let response = ui.interact(rect, id, Sense::CLICK);
    let colour = if row.undone {
        theme.colors.text_dim.0
    } else {
        theme.colors.text.0
    };
    if current {
        // The current row is drawn selected (R4).
        ui.painter()
            .rect_filled(rect, 0.0, theme.colors.lane_selected.0);
    } else if highlighted {
        ui.painter()
            .rect_filled(rect, 0.0, theme.colors.control_hover.0);
    }
    let text = Rect::from_min_max(
        pos2(rect.left() + MARGIN, rect.top()),
        pos2(rect.right() - MARGIN, rect.bottom()),
    );
    // The agent's mark, before the label (R9). Every row keeps the mark's
    // room, so the labels line up.
    let label = Rect::from_min_max(
        pos2(text.left() + MARK, text.top()),
        pos2(text.right(), text.bottom()),
    );
    if let Some(maker) = &row.maker
        && maker.is_agent()
    {
        ui.painter().circle_filled(
            pos2(text.left() + 4.0, text.center().y),
            3.0,
            theme.colors.accent.0,
        );
    }
    paint_label(ui, theme, label, &row.label, colour);
    // The whole label as hover text, or who made the step (R4, R9).
    let hover = match &row.maker {
        Some(maker) if maker.is_agent() => format!("Made by {}", maker.name()),
        _ => row.label.clone(),
    };
    let clicked = response.clicked();
    response.on_hover_text(hover);
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::ListItem);
        node.set_label(row.label.clone());
        node.set_bounds(bounds(rect));
        node.set_selected(current);
        if row.undone {
            node.set_state_description("undone");
        }
        if let Some(maker) = &row.maker
            && maker.is_agent()
        {
            node.set_description(format!("Made by {}", maker.name()));
        }
    });
    clicked
}

/// Paints a row's label, cut short with an ellipsis where it is wider
/// than the row (R4).
fn paint_label(ui: &Ui, theme: &Theme, rect: Rect, text: &str, colour: Color32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), theme.font(), colour);
    if galley.size().x <= rect.width() {
        ui.painter().galley(rect.left_top(), galley, colour);
        return;
    }
    let ellipsis = ui
        .painter()
        .layout_no_wrap("\u{2026}".to_owned(), theme.font(), colour);
    let room = rect.width() - ellipsis.size().x;
    let mut cut = text.to_owned();
    while cut.chars().count() > 1 {
        cut.pop();
        let galley = ui
            .painter()
            .layout_no_wrap(cut.clone(), theme.font(), colour);
        if galley.size().x <= room {
            break;
        }
    }
    let galley = ui
        .painter()
        .layout_no_wrap(format!("{cut}\u{2026}"), theme.font(), colour);
    ui.painter().galley(rect.left_top(), galley, colour);
}

/// Adds a control to the main window's Tab order.
fn register(ui: &Ui, id: Id) {
    crate::focus::register(ui.ctx(), id, Area::UndoHistory, None);
}

/// Names a widget for assistive technology.
fn name(ui: &Ui, id: Id, label: &str) {
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_label(label.to_owned());
    });
}

fn bounds(rect: Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.left().into(),
        y0: rect.top().into(),
        x1: rect.right().into(),
        y1: rect.bottom().into(),
    }
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}
