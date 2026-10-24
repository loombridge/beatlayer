//! The Save History panel (IMP-05, WP-6.39; the crash-recovery spec, R12 to
//! R19): the open set's save points and saved versions, newest first, in the
//! right-hand area where the Undo History opens.
//!
//! Selecting a row and pressing Restore, double-clicking it, or pressing
//! Return with it selected restores it (R16). Only the rows in view are
//! built. With Save Continuously off a line says what it keeps, with a
//! button to the Settings window's File & Folder page (R14).

use std::time::SystemTime;

use bl_project::history::{Kind, Row};
use bl_project::local_stamp;
use bl_shell::commands::{APP_SETTINGS, VIEW_TOGGLE_SAVE_HISTORY};
use bl_shell::{Area, Shell};
use egui::accesskit::Role;
use egui::{Align2, Id, Key, Rect, ScrollArea, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::undo_history_panel::{HEADER, MARGIN, ROW, bounds, invoke, name, paint_label, register};

/// The height of the line that says what Save Continuously keeps, and of its
/// Settings button.
const NOTE: f32 = 62.0;
/// The height of the foot, where Restore is.
const FOOT: f32 = 30.0;
/// The width the time takes at a row's right edge.
const TIME: f32 = 108.0;

/// The panel's state between frames: the row selected, and whether the list
/// takes the keyboard focus.
#[derive(Debug, Default)]
pub struct SaveHistoryPanelUi {
    /// The selected row's `id`.
    selected: Option<String>,
    /// Whether the list takes the keyboard focus.
    focus: bool,
    /// Whether the panel showed last time, so opening and closing it can move
    /// the keyboard focus.
    shown: bool,
}

impl SaveHistoryPanelUi {
    /// Draws the panel in `rect`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        if !self.shown {
            // Opening the panel gives it the keyboard focus, as the Undo
            // History does.
            self.shown = true;
            self.focus = true;
            events.push(UiEvent::FocusArea(Area::UndoHistory));
        }
        // A save point reaches the disk after its edit and a save after its
        // command, so a frame comes by to list them (R15).
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
        let rows = shell.save_history_rows();
        if self
            .selected
            .as_ref()
            .is_some_and(|id| !rows.iter().any(|row| &row.id == id))
        {
            self.selected = None;
        }

        let region = Id::new("region-save-history");
        let _ = ui.interact(rect, region, Sense::hover());
        ui.ctx().accesskit_node_builder(region, |node| {
            node.set_role(Role::Group);
            node.set_label("Save History".to_owned());
            node.set_bounds(bounds(rect));
        });
        ui.painter().rect_filled(rect, 0.0, theme.colors.panel.0);
        ui.scope_builder(
            UiBuilder::new().max_rect(rect).accessibility_parent(region),
            |ui| {
                let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER));
                let note = if shell.save_continuously() { 0.0 } else { NOTE };
                let note = Rect::from_min_size(header.left_bottom(), vec2(rect.width(), note));
                let foot = Rect::from_min_max(pos2(rect.left(), rect.bottom() - FOOT), rect.max);
                let list = Rect::from_min_max(note.left_bottom(), foot.left_top());
                self.header(ui, theme, header, events);
                if note.height() > 0.0 {
                    self.note(ui, theme, note, events);
                }
                self.list(ui, theme, list, rows, events);
                self.foot(ui, foot, rows, events);
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
        self.selected = None;
    }

    /// The title and the close button (L-LRN-002).
    fn header(&mut self, ui: &mut Ui, theme: &Theme, rect: Rect, events: &mut Vec<UiEvent>) {
        let at = Rect::from_min_size(pos2(rect.left() + 4.0, rect.top() + 1.0), vec2(18.0, 18.0));
        let response = ui
            .put(at, egui::Button::new("\u{00d7}").frame(false))
            .on_hover_text("Close Save History");
        name(ui, response.id, "Close Save History");
        register(ui, response.id);
        if response.clicked() {
            invoke(events, VIEW_TOGGLE_SAVE_HISTORY, json!({}));
        }
        ui.painter().text(
            pos2(at.right() + 4.0, rect.center().y - 5.0),
            Align2::LEFT_CENTER,
            "Save History",
            theme.font(),
            theme.colors.text.0,
        );
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
    }

    /// With Save Continuously off: what it keeps, and the way to its switch
    /// (R14). The wording is Beatlayer's own.
    fn note(&mut self, ui: &mut Ui, theme: &Theme, rect: Rect, events: &mut Vec<UiEvent>) {
        let text = Rect::from_min_size(
            rect.min + vec2(MARGIN, 4.0),
            vec2(rect.width() - 2.0 * MARGIN, NOTE - 28.0),
        );
        ui.put(
            text,
            egui::Label::new(
                egui::RichText::new(
                    "Save Continuously keeps a save point after every edit. It is off, so only \
                     saved versions are listed.",
                )
                .color(theme.colors.text_dim.0),
            ),
        );
        let at = Rect::from_min_size(
            pos2(rect.left() + MARGIN, rect.bottom() - 24.0),
            vec2(80.0, 20.0),
        );
        let response = ui.put(at, egui::Button::new("Settings"));
        register(ui, response.id);
        if response.clicked() {
            invoke(events, APP_SETTINGS, json!({ "page": "file-folder" }));
        }
    }

    /// The rows, newest first. Only the rows in view are built.
    fn list(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        rect: Rect,
        rows: &[Row],
        events: &mut Vec<UiEvent>,
    ) {
        let list = Id::new("save-history-list");
        let response = ui.interact(rect, list, Sense::click());
        if self.focus {
            response.request_focus();
        }
        if response.has_focus() {
            self.keys(ui, rows, events);
        }
        ui.ctx().accesskit_node_builder(list, |node| {
            node.set_role(Role::ListBox);
            node.set_label("Save History Rows".to_owned());
            node.set_bounds(bounds(rect));
            if let Some(selected) = &self.selected {
                node.set_active_descendant(
                    Id::new("save-history-row").with(selected).accesskit_id(),
                );
            }
        });
        if rows.is_empty() {
            ui.painter().text(
                pos2(rect.left() + MARGIN, rect.top() + 10.0),
                Align2::LEFT_CENTER,
                "Save the set to start its history.",
                theme.font(),
                theme.colors.text_dim.0,
            );
            return;
        }
        let now = local_stamp(SystemTime::now());
        let mut area = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(MARGIN, 2.0)))
                .id_salt("save-history-scroll")
                .accessibility_parent(list),
        );
        area.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical()
            .id_salt("save-history-rows")
            .show_rows(&mut area, ROW, rows.len(), |ui, range| {
                let first = range.start;
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
                    let selected = self.selected.as_deref() == Some(row.id.as_str());
                    let (clicked, double) = draw_row(ui, theme, at, row, &now, selected);
                    if clicked {
                        self.focus = true;
                        self.selected = Some(row.id.clone());
                    }
                    if double {
                        restore(events, &row.id);
                    }
                }
            });
    }

    /// Up and Down move the selection, Return restores it, and Esc gives the
    /// focus back to the main view (R16, R19).
    fn keys(&mut self, ui: &mut Ui, rows: &[Row], events: &mut Vec<UiEvent>) {
        let step =
            ui.input(|i| i.key_pressed(Key::ArrowDown) as i8 - i.key_pressed(Key::ArrowUp) as i8);
        if step != 0 && !rows.is_empty() {
            let at = self
                .selected
                .as_ref()
                .and_then(|id| rows.iter().position(|row| &row.id == id));
            let next = match (at, step > 0) {
                (None, _) => 0,
                (Some(at), true) => (at + 1).min(rows.len() - 1),
                (Some(at), false) => at.saturating_sub(1),
            };
            self.selected = Some(rows[next].id.clone());
        }
        if ui.input(|i| i.key_pressed(Key::Enter))
            && let Some(id) = &self.selected
        {
            restore(events, id);
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            self.focus = false;
            events.push(UiEvent::FocusArea(Area::MainView));
        }
    }

    /// Restore, at the foot (R16).
    fn foot(&mut self, ui: &mut Ui, rect: Rect, rows: &[Row], events: &mut Vec<UiEvent>) {
        let at = Rect::from_min_size(
            pos2(rect.left() + MARGIN, rect.top() + 5.0),
            vec2(80.0, 20.0),
        );
        let selected = self
            .selected
            .as_ref()
            .filter(|id| rows.iter().any(|row| &row.id == *id));
        let response = ui
            .add_enabled_ui(selected.is_some(), |ui| {
                ui.put(at, egui::Button::new("Restore"))
            })
            .inner;
        register(ui, response.id);
        if response.clicked()
            && let Some(id) = selected
        {
            restore(events, id);
        }
    }
}

/// Runs `set.restore` for a row.
fn restore(events: &mut Vec<UiEvent>, id: &str) {
    invoke(events, bl_project::history::RESTORE, json!({ "id": id }));
}

/// Draws one row: its label, cut short where it is wider than the row, and
/// its time at the right. Says whether it was clicked and double-clicked.
fn draw_row(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    row: &Row,
    now: &str,
    selected: bool,
) -> (bool, bool) {
    let id = Id::new("save-history-row").with(&row.id);
    let response = ui.interact(rect, id, Sense::click());
    if selected {
        ui.painter()
            .rect_filled(rect, 0.0, theme.colors.lane_selected.0);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, theme.colors.control_hover.0);
    }
    let time = time_text(row, now);
    let text = Rect::from_min_max(
        pos2(rect.left() + MARGIN, rect.top()),
        pos2(rect.right() - MARGIN, rect.bottom()),
    );
    let label = Rect::from_min_max(text.min, pos2(text.right() - TIME, text.bottom()));
    paint_label(ui, theme, label, &row.label, theme.colors.text.0);
    ui.painter().text(
        text.right_center(),
        Align2::RIGHT_CENTER,
        &time,
        theme.font(),
        theme.colors.text_dim.0,
    );
    let (clicked, double) = (response.clicked(), response.double_clicked());
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::ListItem);
        node.set_label(row.label.clone());
        node.set_description(time);
        node.set_bounds(bounds(rect));
        node.set_selected(selected);
    });
    (clicked, double)
}

/// A row's time in local time: with seconds for a save point, and with the
/// date when it is not of today (R13). `now` is [`local_stamp`] of the
/// present.
fn time_text(row: &Row, now: &str) -> String {
    let stamp = local_stamp(row.time);
    let (date, clock) = stamp.split_once(' ').unwrap_or((&stamp, "000000"));
    let clock = if row.kind == Kind::SavePoint {
        format!("{}:{}:{}", &clock[..2], &clock[2..4], &clock[4..6])
    } else {
        format!("{}:{}", &clock[..2], &clock[2..4])
    };
    if now.starts_with(date) {
        clock
    } else {
        format!("{date} {clock}")
    }
}
