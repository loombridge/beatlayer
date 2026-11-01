//! The Filter View, the Results bar and the Filter View Menu (spec:
//! browser-tags R18 to R23): the tag groups at the top of the content pane
//! with their wrapping tag buttons, and what a search or a filter has
//! selected.

use std::hash::{Hash, Hasher};

use bl_shell::Shell;
use bl_shell::browser_commands::{self as ids, FilterGroup, FilterViewState};
use egui::accesskit::{Role, Toggled};
use egui::{Align2, Id, Key, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use serde_json::json;

use super::{BrowserUi, Glyph, bounds, command, glyph, invoke};
use crate::layout::{BROWSER_ROW, BrowserLayout};
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The Filter View's area.
#[must_use]
pub fn filter_view_id() -> Id {
    Id::new("browser-filter-view")
}

/// A group in the Filter View, by its name.
#[must_use]
pub fn filter_group_id(name: &str) -> Id {
    Id::new(("browser-filter-group", name))
}

/// A tag button in the Filter View, by the tag's full name.
#[must_use]
pub fn filter_tag_id(full_name: &str) -> Id {
    Id::new(("browser-filter-tag", full_name))
}

/// The Results bar.
#[must_use]
pub fn results_bar_id() -> Id {
    Id::new("browser-results-bar")
}

/// The Results bar's Clear button.
#[must_use]
pub fn clear_filters_id() -> Id {
    Id::new("browser-clear-filters")
}

/// The Results bar's Add Label button.
#[must_use]
pub fn add_label_id() -> Id {
    Id::new("browser-add-label")
}

/// The room around a tag button's name, in points.
const BUTTON_PAD: f32 = 12.0;
/// The gap between two tag buttons.
const BUTTON_GAP: f32 = 3.0;
/// A tag button's height.
const BUTTON_HEIGHT: f32 = 15.0;

/// One line of the Filter View: a group's name, or a row of its tag
/// buttons.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    /// The group it belongs to, by its place among the groups.
    group: usize,
    /// The buttons on it: the tag's place in its group, its left edge from
    /// the area's, and its width. None for a group's name.
    buttons: Vec<(usize, f32, f32)>,
}

/// The Filter View's lines for one set of groups and one width, which
/// stay as they are until either changes.
#[derive(Clone, Debug, Default)]
pub(super) struct FilterLines {
    key: u64,
    lines: Vec<Line>,
}

impl BrowserUi {
    /// Works out the lines of the groups' tag buttons for this width,
    /// unless they are as they were.
    fn filter_lines(
        &mut self,
        ui: &Ui,
        theme: &Theme,
        groups: &[FilterGroup],
        width: f32,
    ) -> &[Line] {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        width.to_bits().hash(&mut hasher);
        for group in groups {
            group.name.hash(&mut hasher);
            for tag in &group.tags {
                tag.name.hash(&mut hasher);
            }
        }
        let key = hasher.finish();
        if self.filter_lines.key != key || self.filter_lines.lines.is_empty() {
            let font = theme.small_font();
            let mut lines = Vec::new();
            for (index, group) in groups.iter().enumerate() {
                lines.push(Line {
                    group: index,
                    buttons: Vec::new(),
                });
                let mut row: Vec<(usize, f32, f32)> = Vec::new();
                let mut x = 4.0;
                for (place, tag) in group.tags.iter().enumerate() {
                    let text = ui.painter().layout_no_wrap(
                        tag.name.clone(),
                        font.clone(),
                        egui::Color32::WHITE,
                    );
                    let w = text.size().x + BUTTON_PAD;
                    if x + w > width - 4.0 && !row.is_empty() {
                        lines.push(Line {
                            group: index,
                            buttons: std::mem::take(&mut row),
                        });
                        x = 4.0;
                    }
                    row.push((place, x, w));
                    x += w + BUTTON_GAP;
                }
                if !row.is_empty() {
                    lines.push(Line {
                        group: index,
                        buttons: row,
                    });
                }
            }
            self.filter_lines = FilterLines { key, lines };
        }
        &self.filter_lines.lines
    }

    /// Draws the Filter View at the content pane's top (L-BRW-007): each
    /// group's name over its wrapping tag buttons. Only the lines on screen
    /// are built (spec: browser-tags R33).
    pub(super) fn filter_view(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &BrowserLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let area = layout.filter_view;
        let view = shell.browser_view();
        let state: FilterViewState = view.filter_view();
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, theme.colors.panel.0);
        let region = ui.interact(area, filter_view_id(), Sense::click());
        crate::focus::register(ui.ctx(), filter_view_id(), bl_shell::Area::Browser, None);
        ui.ctx().accesskit_node_builder(filter_view_id(), |node| {
            node.set_role(Role::Group);
            node.set_label("Filter View".to_owned());
            node.set_bounds(bounds(area));
        });
        if region.clicked() {
            region.request_focus();
        }
        let groups = state.groups.clone();
        let width = area.width();
        let lines = self.filter_lines(ui, theme, &groups, width).to_vec();
        let content = lines.len() as f32 * BROWSER_ROW;
        self.filter_scroll = super::scrolled(ui, area, self.filter_scroll, content);
        let first = (self.filter_scroll / BROWSER_ROW).floor() as usize;
        let visible = ((area.height() / BROWSER_ROW).ceil() as usize + 1)
            .min(lines.len().saturating_sub(first));

        // The tag buttons in the order the arrow keys walk them.
        let order: Vec<(usize, usize)> = lines
            .iter()
            .flat_map(|line| line.buttons.iter().map(|&(place, ..)| (line.group, place)))
            .collect();
        let focused = ui.memory(|m| m.has_focus(filter_view_id()));
        if focused && !order.is_empty() {
            self.filter_cursor = self.filter_cursor.min(order.len() - 1);
            self.filter_keys(ui, shell, &groups, &lines, &order, events);
        }

        let mut clip_ui = ui.new_child(UiBuilder::new().max_rect(area));
        clip_ui.set_clip_rect(area);
        let mut noted: Vec<usize> = Vec::new();
        for (offset, line) in lines.iter().enumerate().skip(first).take(visible) {
            let top = area.top() + offset as f32 * BROWSER_ROW - self.filter_scroll;
            let row = Rect::from_min_size(pos2(area.left(), top), vec2(area.width(), BROWSER_ROW));
            let group = &groups[line.group];
            let group_id = filter_group_id(&group.name);
            if !noted.contains(&line.group) {
                noted.push(line.group);
                // Each group reaches assistive technology with its name.
                let _ = clip_ui.interact(row, group_id, Sense::hover());
                ui.ctx().accesskit_node_builder(group_id, |node| {
                    node.set_role(Role::Group);
                    node.set_label(group.name.clone());
                    node.set_bounds(bounds(row));
                });
            }
            if line.buttons.is_empty() {
                // The group's name, with its context menu (R21).
                let heading = clip_ui.interact(row, group_id.with("name"), Sense::click());
                painter.text(
                    pos2(row.left() + 4.0, row.center().y),
                    Align2::LEFT_CENTER,
                    &group.name,
                    theme.small_font(),
                    theme.colors.text_dim.0,
                );
                heading.context_menu(|ui| {
                    let item = ui.button("Hide Filter Group");
                    crate::menus::register_item(ui.ctx(), &item);
                    crate::menus::take_focus_first(ui, &item);
                    if item.clicked() {
                        invoke(
                            events,
                            ids::SET_FILTER_GROUPS,
                            json!({ "group": group.name, "shown": false }),
                        );
                        ui.close();
                    }
                });
                continue;
            }
            let mut tags_ui = clip_ui.new_child(
                UiBuilder::new()
                    .max_rect(row)
                    .accessibility_parent(group_id),
            );
            tags_ui.set_clip_rect(area);
            for &(place, x, w) in &line.buttons {
                let tag = &group.tags[place];
                let rect = Rect::from_min_size(
                    pos2(row.left() + x, row.center().y - BUTTON_HEIGHT / 2.0),
                    vec2(w, BUTTON_HEIGHT),
                );
                let id = filter_tag_id(&tag.full_name);
                let response = tags_ui.interact(rect, id, Sense::click());
                let at_cursor =
                    focused && order.get(self.filter_cursor) == Some(&(line.group, place));
                let fill = if tag.selected {
                    theme.colors.accent.0
                } else if response.hovered() {
                    theme.colors.control_hover.0
                } else {
                    theme.colors.control.0
                };
                painter.rect_filled(rect, theme.sizes.corner, fill);
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    &tag.name,
                    theme.small_font(),
                    if tag.selected {
                        theme.colors.accent_text.0
                    } else {
                        theme.colors.text.0
                    },
                );
                if at_cursor && crate::focus::outlined(ui.ctx()) {
                    painter.rect_stroke(
                        rect.expand(1.0),
                        theme.sizes.corner,
                        Stroke::new(1.0, theme.colors.focus.0),
                        StrokeKind::Outside,
                    );
                }
                if response.clicked() {
                    // Spec: browser-tags R20, I-CLICK-031. A click selects
                    // the tag alone in its group; Cmd-click (Ctrl-click)
                    // adds it or takes it out.
                    let add = command(ui.input(|i| i.modifiers));
                    if let Some(at) = order.iter().position(|&o| o == (line.group, place)) {
                        self.filter_cursor = at;
                    }
                    ui.memory_mut(|m| m.request_focus(filter_view_id()));
                    invoke(
                        events,
                        ids::FILTER,
                        json!({ "tags": [tag.full_name], "add": add }),
                    );
                }
                let selected = tag.selected;
                let name = tag.full_name.clone();
                ui.ctx().accesskit_node_builder(id, |node| {
                    node.set_role(Role::Button);
                    node.set_label(name);
                    node.set_toggled(if selected {
                        Toggled::True
                    } else {
                        Toggled::False
                    });
                    node.set_bounds(bounds(rect));
                });
            }
        }
        // Spec: accessibility R7: outlined only with Outline View in Focus.
        if focused && crate::focus::outlined(ui.ctx()) {
            ui.painter().rect_stroke(
                area.shrink(1.0),
                0.0,
                Stroke::new(1.0, theme.colors.focus.0),
                StrokeKind::Inside,
            );
        }
    }

    /// The keys the Filter View reads (spec: browser-tags R23): the arrow
    /// keys move between tag buttons, Enter selects or clears one alone,
    /// and Cmd-Enter (Ctrl-Enter) adds it to its group's selection or takes
    /// it out.
    fn filter_keys(
        &mut self,
        ui: &mut Ui,
        _shell: &Shell,
        groups: &[FilterGroup],
        lines: &[Line],
        order: &[(usize, usize)],
        events: &mut Vec<UiEvent>,
    ) {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                filter_view_id(),
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..egui::EventFilter::default()
                },
            );
        });
        let (left, right, up, down) = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
            )
        });
        // Enter with the modifiers held as it went down.
        let enter = ui.input(|i| {
            i.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    modifiers,
                    ..
                } => Some(*modifiers),
                _ => None,
            })
        });
        if left || right || up || down {
            ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
        }
        let at = self.filter_cursor;
        if left {
            self.filter_cursor = at.saturating_sub(1);
        } else if right {
            self.filter_cursor = (at + 1).min(order.len() - 1);
        } else if up || down {
            // To the button nearest across on the line above or below.
            let here = lines
                .iter()
                .position(|line| {
                    line.group == order[at].0
                        && line.buttons.iter().any(|&(place, ..)| place == order[at].1)
                })
                .unwrap_or(0);
            let x = lines[here]
                .buttons
                .iter()
                .find(|&&(place, ..)| place == order[at].1)
                .map_or(0.0, |&(_, x, _)| x);
            let target = if up {
                (0..here).rev().find(|&l| !lines[l].buttons.is_empty())
            } else {
                (here + 1..lines.len()).find(|&l| !lines[l].buttons.is_empty())
            };
            if let Some(line) = target.map(|l| &lines[l])
                && let Some(&(place, ..)) = line.buttons.iter().min_by(|a, b| {
                    (a.1 - x)
                        .abs()
                        .partial_cmp(&(b.1 - x).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                && let Some(index) = order.iter().position(|&o| o == (line.group, place))
            {
                self.filter_cursor = index;
            }
        }
        if let Some(modifiers) = enter
            && let Some(&(group, place)) = order.get(self.filter_cursor)
        {
            let tag = &groups[group].tags[place];
            invoke(
                events,
                ids::FILTER,
                json!({ "tags": [tag.full_name], "add": command(modifiers) }),
            );
        }
    }

    /// The Results bar (L-BRW-008; spec: browser-tags R22): how many filter
    /// tags are selected, Clear, which clears them and the search words,
    /// and Add Label, which keeps what the browser shows as a saved label.
    pub(super) fn results_bar(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &BrowserLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let bar = layout.results_bar;
        let view = shell.browser_view();
        let count = view.filter_count();
        let painter = ui.painter_at(bar);
        painter.rect_filled(bar, 0.0, theme.colors.ruler.0);
        let text = match count {
            0 => "No filters".to_owned(),
            1 => "1 filter".to_owned(),
            n => format!("{n} filters"),
        };
        painter.text(
            pos2(bar.left() + 6.0, bar.center().y),
            Align2::LEFT_CENTER,
            &text,
            theme.small_font(),
            theme.colors.text_dim.0,
        );
        let _ = ui.interact(bar, results_bar_id(), Sense::hover());
        ui.ctx().accesskit_node_builder(results_bar_id(), |node| {
            node.set_role(Role::Group);
            node.set_label("Results".to_owned());
            node.set_value(text.clone());
            node.set_bounds(bounds(bar));
        });
        // Add Label (a bookmark) and Clear (an X), at the bar's right.
        let size = vec2(BROWSER_ROW, BROWSER_ROW);
        let clear = Rect::from_min_size(
            pos2(bar.right() - 4.0 - size.x, bar.center().y - size.y / 2.0),
            size,
        );
        let add = clear.translate(vec2(-size.x - 2.0, 0.0));
        for (rect, id, name, kind) in [
            (add, add_label_id(), "Add Label", Glyph::Bookmark),
            (clear, clear_filters_id(), "Clear", Glyph::Cross),
        ] {
            let response = ui.interact(rect, id, Sense::click());
            crate::focus::register(ui.ctx(), id, bl_shell::Area::Browser, None);
            if response.hovered() {
                painter.rect_filled(rect, theme.sizes.corner, theme.colors.control_hover.0);
            }
            glyph(&painter, rect, kind, theme.colors.text.0);
            if response.clicked() {
                if id == add_label_id() {
                    invoke(events, ids::ADD_LABEL, json!({}));
                } else {
                    invoke(events, ids::CLEAR_FILTERS, json!({}));
                }
            }
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Button);
                node.set_label(name.to_owned());
                node.set_bounds(bounds(rect));
            });
        }
    }

    /// The Filter View Menu (C-FILTER; spec: browser-tags R21): the groups
    /// the chosen label can show, ticked while they show, Show All Filter
    /// Groups, Reset Filter Groups to Default, Include Auto Tags, the Tag
    /// Editor and Show Quick Tags.
    pub(super) fn filter_view_menu(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.browser_view();
        let state = view.filter_view();
        let mut first = true;
        let mut item = |ui: &mut Ui, text: String, enabled: bool| -> egui::Response {
            let response = ui.add_enabled(enabled, egui::Button::new(text));
            crate::menus::register_item(ui.ctx(), &response);
            if first && enabled {
                crate::menus::take_focus_first(ui, &response);
                first = false;
            }
            response
        };
        for group in &state.menu {
            let mark = if group.shown { "\u{2714} " } else { "" };
            if item(ui, format!("{mark}{}", group.name), true).clicked() {
                invoke(
                    events,
                    ids::SET_FILTER_GROUPS,
                    json!({ "group": group.name, "shown": !group.shown }),
                );
                ui.close();
            }
        }
        ui.separator();
        if item(ui, "Show All Filter Groups".to_owned(), true).clicked() {
            invoke(events, ids::SET_FILTER_GROUPS, json!({ "show_all": true }));
            ui.close();
        }
        if item(ui, "Reset Filter Groups to Default".to_owned(), true).clicked() {
            invoke(events, ids::SET_FILTER_GROUPS, json!({ "reset": true }));
            ui.close();
        }
        ui.separator();
        // Spec: browser-tags R55. Ticked while automatic tags show, filter
        // and answer `#` search.
        let include = state.include_auto_tags;
        let mark = if include { "\u{2714} " } else { "" };
        if item(ui, format!("{mark}Include Auto Tags"), true).clicked() {
            invoke(
                events,
                ids::SET_INCLUDE_AUTO_TAGS,
                json!({ "include": !include }),
            );
            ui.close();
        }
        let tag_editor = view.tag_editor();
        let mark = if tag_editor { "\u{2714} " } else { "" };
        if item(ui, format!("{mark}Tag Editor"), true).clicked() {
            invoke(events, ids::SHOW_TAG_EDITOR, json!({ "show": !tag_editor }));
            ui.close();
        }
        let quick = view.quick_tags_shown();
        let mark = if quick { "\u{2714} " } else { "" };
        if item(ui, format!("{mark}Show Quick Tags"), true).clicked() {
            invoke(events, ids::SHOW_QUICK_TAGS, json!({ "show": !quick }));
            ui.close();
        }
    }
}
