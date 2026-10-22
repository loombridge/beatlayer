//! The search field (WP-6.42, IMP-16 and IMP-18): one field over the main
//! view with a Commands scope and a Tracks scope, its list of rows, and the
//! outline the Tracks scope puts round the title bars of the tracks it
//! finds. The behaviour is in the command palette spec, R1 to R14.
//!
//! The field is the shell's state (`Shell::search`): this draws it and turns
//! typing, Up, Down, Return, Shift-Return, Esc and clicks into the
//! `search.*` commands, or into [`UiEvent::SearchRun`] for choosing a row.

use bl_shell::search::Field;
use bl_shell::search_commands::{CommandRow, SearchScope, TrackRow};
use bl_shell::{Invocation, Shell};
use egui::accesskit::{Live, Role, Toggled};
use egui::{
    Align2, Context, Id, Key, Modifiers, Order, Rect, Sense, Stroke, StrokeKind, TextEdit, Ui,
    pos2, vec2,
};
use serde_json::{Value, json};

use crate::keymap::{Platform, chord_text, parse_binding};
use crate::layout::{SEARCH_HEADER, SEARCH_ROW, SEARCH_ROWS, search_field_rect};
use crate::main_window::{InfoViewContent, UiEvent};
use crate::theme::Theme;

/// The margin inside the field.
const MARGIN: f32 = 6.0;
/// The width of a scope button.
const SCOPE_WIDTH: f32 = 72.0;
/// The width of the Show Only Matches switch.
const SWITCH_WIDTH: f32 = 128.0;

/// The ID of the field's panel.
#[must_use]
pub fn field_id() -> Id {
    Id::new("search-field")
}

/// The ID of the text being typed.
#[must_use]
pub fn text_id() -> Id {
    Id::new("search-field-text")
}

/// The ID of the list of rows.
#[must_use]
pub fn list_id() -> Id {
    Id::new("search-field-list")
}

/// The ID of a row of the list.
#[must_use]
pub fn row_id(index: usize) -> Id {
    Id::new(("search-field-row", index))
}

/// The field's state between frames.
#[derive(Debug, Default)]
pub struct SearchFieldUi {
    /// Whether the last frame drew the field open.
    shown: bool,
    /// The scope the last frame drew, so switching it keeps the keyboard in
    /// the text.
    scope: Option<SearchScope>,
    /// What had the keyboard focus before the field opened, which gets it
    /// back when the field closes (R3).
    restore: Option<Id>,
    /// Where the last frame drew the field, so a click in it isn't a click
    /// in the pane under it.
    rect: Option<Rect>,
    /// The highlighted row the last frame drew, so a change scrolls it into
    /// view.
    scrolled_to: Option<usize>,
    /// The track the last frame had selected, so a new selection through the
    /// field brings its column into view in the mixers (R11).
    selected: Option<bl_model::TrackId>,
    /// The field's controls the last frame drew, in the order Tab walks
    /// them (R13), and the ones this frame has drawn so far.
    order: Vec<Id>,
    drawing: Vec<Id>,
}

impl SearchFieldUi {
    /// Whether the field was open when the last frame ended: while it is, key
    /// presses go to it as to a text field.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.shown
    }

    /// Whether the field covers this point.
    #[must_use]
    pub fn contains(&self, pos: egui::Pos2) -> bool {
        self.rect.is_some_and(|rect| rect.contains(pos))
    }

    /// The track the field just selected, whose column the mixer in front
    /// should bring into view (R11): the selection changed while the Tracks
    /// scope was open.
    pub fn revealed(&mut self, shell: &Shell) -> Option<bl_model::TrackId> {
        let selected = shell.arrangement().selected_tracks().last().copied();
        let tracks = shell.search().open && shell.search().scope == SearchScope::Tracks;
        let changed = std::mem::replace(&mut self.selected, selected) != selected;
        (changed && tracks).then_some(selected).flatten()
    }

    /// Where Tab (or Shift-Tab) goes from the control with the keyboard: the
    /// next control of the field, round to the first (R13).
    #[must_use]
    pub fn tab(&self, ctx: &Context, backwards: bool) -> Option<Id> {
        let len = self.order.len();
        let at = ctx
            .memory(egui::Memory::focused)
            .and_then(|focused| self.order.iter().position(|id| *id == focused));
        let next = match (at, backwards) {
            (Some(at), false) => (at + 1) % len.max(1),
            (Some(at), true) => (at + len.max(1) - 1) % len.max(1),
            (None, false) => 0,
            (None, true) => len.saturating_sub(1),
        };
        self.order.get(next).copied()
    }

    /// Draws the field over `main_view`, if the shell has it open, and adds
    /// what the person did to `events`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        main_view: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let ctx = ui.ctx().clone();
        let field = shell.search();
        if !field.open {
            if self.shown {
                // Spec: command-palette R3. The keyboard goes back to where
                // it was.
                if let Some(id) = self.restore.take() {
                    ctx.memory_mut(|m| m.request_focus(id));
                }
            }
            *self = Self::default();
            return;
        }
        let opened = !self.shown;
        if opened {
            self.restore = ctx.memory(egui::Memory::focused);
        }
        let rows = list_len(field);
        let rect = search_field_rect(main_view, rows.max(usize::from(no_match(field))));
        self.rect = Some(rect);
        let mut closing = false;
        egui::Area::new(field_id())
            .order(Order::Foreground)
            .fixed_pos(rect.min)
            .constrain(false)
            .show(&ctx, |ui| {
                ui.allocate_rect(rect, Sense::hover());
                self.draw(ui, shell, theme, rect, opened, &mut closing, events);
            });
        // Spec: command-palette R3. A click outside the field closes it.
        let outside = ctx.input(|i| {
            i.pointer
                .any_pressed()
                .then(|| i.pointer.press_origin())
                .flatten()
                .is_some_and(|pos| !rect.contains(pos))
        });
        if (outside || closing) && !opened {
            invoke(events, bl_shell::search_commands::CLOSE, json!({}));
        }
        // Tab is the field's own while one of its controls has the keyboard:
        // egui would otherwise take it before `draw` sees it.
        for id in &self.drawing {
            ctx.memory_mut(|m| {
                m.set_focus_lock_filter(
                    *id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
        }
        self.order = std::mem::take(&mut self.drawing);
        self.shown = true;
        self.scope = Some(field.scope);
    }

    #[expect(clippy::too_many_arguments, reason = "one panel, drawn in full")]
    fn draw(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        opened: bool,
        closing: &mut bool,
        events: &mut Vec<UiEvent>,
    ) {
        let field = shell.search();
        let colors = &theme.colors;
        // Spec: command-palette R13. Tab moves round the field's controls
        // and stays in the field.
        // Shift-Tab first: a plain Tab also matches with Shift held.
        for backwards in [true, false] {
            let mods = if backwards {
                Modifiers::SHIFT
            } else {
                Modifiers::NONE
            };
            if self.order.iter().any(|id| ui.memory(|m| m.has_focus(*id)))
                && ui.input_mut(|i| i.consume_key(mods, Key::Tab))
                && let Some(id) = self.tab(ui.ctx(), backwards)
            {
                ui.memory_mut(|m| m.request_focus(id));
            }
        }
        ui.painter()
            .rect_filled(rect, theme.sizes.corner, colors.panel.0);
        ui.painter().rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.0, colors.separator.0),
            StrokeKind::Inside,
        );
        let region = Id::new("search-field-region");
        let title = match field.scope {
            SearchScope::Commands => "Search Commands",
            SearchScope::Tracks => "Find Track",
        };
        ui.ctx().accesskit_node_builder(region, |node| {
            node.set_role(Role::Group);
            node.set_label(title.to_owned());
            node.set_bounds(bounds(rect));
        });

        // The top row: the scope buttons, the text, and in the Tracks scope
        // Show Only Matches.
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), SEARCH_HEADER));
        let mut left = header.left() + MARGIN;
        for scope in [SearchScope::Commands, SearchScope::Tracks] {
            let at = Rect::from_min_size(
                pos2(left, header.top() + 4.0),
                vec2(SCOPE_WIDTH, SEARCH_HEADER - 8.0),
            );
            left += SCOPE_WIDTH + 2.0;
            let on = field.scope == scope;
            let label = match scope {
                SearchScope::Commands => "Commands",
                SearchScope::Tracks => "Tracks",
            };
            let id = Id::new(("search-field-scope", scope == SearchScope::Tracks));
            if self.toggle(ui, theme, at, id, label, Role::Button, on) && !on {
                invoke(
                    events,
                    bl_shell::search_commands::SET_SCOPE,
                    json!({ "scope": scope }),
                );
            }
        }
        let switch = (field.scope == SearchScope::Tracks).then(|| {
            Rect::from_min_size(
                pos2(header.right() - MARGIN - SWITCH_WIDTH, header.top() + 4.0),
                vec2(SWITCH_WIDTH, SEARCH_HEADER - 8.0),
            )
        });
        let text_rect = Rect::from_min_max(
            pos2(left + 4.0, header.top() + 4.0),
            pos2(
                switch.map_or(header.right() - MARGIN, |s| s.left() - 4.0),
                header.bottom() - 4.0,
            ),
        );
        self.text(ui, shell, theme, text_rect, title, opened, events);
        if let Some(at) = switch {
            let on = field.only_matches;
            let id = Id::new("search-field-only-matches");
            if self.toggle(ui, theme, at, id, "Show Only Matches", Role::Switch, on) {
                invoke(
                    events,
                    bl_shell::search_commands::SET_ONLY_MATCHES,
                    json!({ "on": !on }),
                );
            }
        }
        ui.painter().line_segment(
            [header.left_bottom(), header.right_bottom()],
            Stroke::new(1.0, colors.separator.0),
        );

        // The list.
        let list = Rect::from_min_max(pos2(rect.left(), header.bottom()), rect.max);
        self.list(ui, shell, theme, list, events);
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            *closing = true;
        }
    }

    /// Draws a toggle button, or the Show Only Matches switch, and says
    /// whether it was pressed, by a click or by Return on it.
    #[expect(clippy::too_many_arguments, reason = "one control, drawn in full")]
    fn toggle(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        at: Rect,
        id: Id,
        label: &str,
        role: Role,
        on: bool,
    ) -> bool {
        let colors = &theme.colors;
        let response = ui.interact(at, id, Sense::click());
        self.drawing.push(id);
        let fill = if on {
            colors.control_pressed.0
        } else if response.hovered() {
            colors.control_hover.0
        } else {
            colors.control.0
        };
        ui.painter().rect_filled(at, theme.sizes.corner, fill);
        if response.has_focus() {
            ui.painter().rect_stroke(
                at,
                theme.sizes.corner,
                Stroke::new(1.0, colors.focus.0),
                StrokeKind::Inside,
            );
        }
        ui.painter().text(
            at.center(),
            Align2::CENTER_CENTER,
            label,
            theme.small_font(),
            colors.text.0,
        );
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(role);
            node.set_label(label.to_owned());
            node.set_toggled(if on { Toggled::True } else { Toggled::False });
            node.set_bounds(bounds(at));
        });
        response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
    }

    /// The text being typed, and the keys that act on the list from it.
    #[expect(clippy::too_many_arguments, reason = "one field, drawn in full")]
    fn text(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        title: &str,
        opened: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let field = shell.search();
        let ctx = ui.ctx().clone();
        let focused = ctx.memory(|m| m.has_focus(text_id()));
        if focused {
            self.keys(ui, shell, events);
        }
        self.drawing.push(text_id());
        let mut text = field.text.clone();
        let response = ui.put(
            rect,
            TextEdit::singleline(&mut text)
                .id(text_id())
                .font(theme.small_font())
                .hint_text(title)
                .desired_width(rect.width()),
        );
        ctx.accesskit_node_builder(text_id(), |node| {
            node.set_role(Role::SearchInput);
            node.set_label(title.to_owned());
        });
        if opened || self.scope != Some(field.scope) {
            response.request_focus();
        }
        if response.changed() {
            invoke(
                events,
                bl_shell::search_commands::SET_TEXT,
                json!({ "text": text }),
            );
        }
    }

    /// Up and Down move the highlight of the Commands scope (R8); Return
    /// chooses the highlighted row, or in the Tracks scope selects the next
    /// match, and Shift-Return the previous (R11); Esc closes the field.
    fn keys(&mut self, ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
        let field = shell.search();
        let commands = field.scope == SearchScope::Commands;
        let take = |ui: &Ui, mods: Modifiers, key: Key| ui.input_mut(|i| i.consume_key(mods, key));
        if commands {
            if take(ui, Modifiers::NONE, Key::ArrowDown) {
                invoke(events, bl_shell::search_commands::NEXT, json!({}));
            }
            if take(ui, Modifiers::NONE, Key::ArrowUp) {
                invoke(events, bl_shell::search_commands::PREVIOUS, json!({}));
            }
        }
        // Shift-Return first: a plain Return also matches with Shift held.
        if take(ui, Modifiers::SHIFT, Key::Enter) {
            if !commands {
                invoke(events, bl_shell::search_commands::PREVIOUS, json!({}));
            }
        } else if take(ui, Modifiers::NONE, Key::Enter) {
            if commands {
                if !field.commands.is_empty() {
                    events.push(UiEvent::SearchRun(field.highlight));
                }
            } else {
                invoke(events, bl_shell::search_commands::NEXT, json!({}));
            }
        }
    }

    /// The list of rows, or the line that says there are none.
    fn list(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let field = shell.search();
        let colors = &theme.colors;
        let count = list_len(field);
        let name = match field.scope {
            SearchScope::Commands => "Commands",
            SearchScope::Tracks => "Tracks",
        };
        // The list box in the accessibility tree, with its rows in it, and a
        // line that announces how many there are each time that changes
        // (R13).
        let list = list_id();
        let highlighted = shell.search_highlight();
        ui.ctx().accesskit_node_builder(list, |node| {
            node.set_role(Role::ListBox);
            node.set_label(name.to_owned());
            node.set_bounds(bounds(rect));
            if let Some(row) = highlighted {
                node.set_active_descendant(row_id(row).accesskit_id());
            }
        });
        let announce = Id::new("search-field-count");
        let text = match (count, field.text.is_empty()) {
            (_, true) => String::new(),
            (0, false) => "No results".to_owned(),
            (1, false) => "1 result".to_owned(),
            (n, false) => format!("{n} results"),
        };
        ui.ctx().accesskit_node_builder(announce, |node| {
            node.set_role(Role::Status);
            node.set_live(Live::Polite);
            node.set_label(text);
            node.set_bounds(bounds(rect));
        });
        if count == 0 {
            if no_match(field) {
                ui.painter().text(
                    pos2(rect.left() + MARGIN + 2.0, rect.top() + SEARCH_ROW / 2.0),
                    Align2::LEFT_CENTER,
                    "No tracks match",
                    theme.small_font(),
                    colors.text_dim.0,
                );
            }
            return;
        }
        let height = rect.height().min(SEARCH_ROWS as f32 * SEARCH_ROW);
        let area = Rect::from_min_size(rect.min, vec2(rect.width(), height));
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("search-field-rows")
            .auto_shrink([false, false]);
        if highlighted != self.scrolled_to
            && let Some(row) = highlighted
        {
            scroll = scroll
                .vertical_scroll_offset((row as f32 * SEARCH_ROW + SEARCH_ROW - height).max(0.0));
        }
        self.scrolled_to = highlighted;
        let mut inner = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(area)
                .accessibility_parent(list),
        );
        inner.set_clip_rect(area);
        inner.spacing_mut().item_spacing.y = 0.0;
        scroll.show_rows(&mut inner, SEARCH_ROW, count, |ui, range| {
            let (left, top, width) = (
                ui.max_rect().left(),
                ui.max_rect().top(),
                ui.max_rect().width(),
            );
            for index in range.clone() {
                let at = Rect::from_min_size(
                    pos2(left, top + (index - range.start) as f32 * SEARCH_ROW),
                    vec2(width, SEARCH_ROW),
                );
                let clicked = match field.scope {
                    SearchScope::Commands => command_row(
                        ui,
                        theme,
                        at,
                        index,
                        &field.commands[index],
                        highlighted == Some(index),
                    ),
                    SearchScope::Tracks => track_row(
                        ui,
                        theme,
                        at,
                        index,
                        &field.tracks[index],
                        highlighted == Some(index),
                    ),
                };
                if clicked {
                    events.push(UiEvent::SearchRun(index));
                }
            }
        });
    }
}

/// How many rows the list has.
fn list_len(field: &Field) -> usize {
    match field.scope {
        SearchScope::Commands => field.commands.len(),
        SearchScope::Tracks => field.tracks.len(),
    }
}

/// Whether the Tracks scope has text that matches nothing: its list says so
/// (R9).
fn no_match(field: &Field) -> bool {
    field.scope == SearchScope::Tracks && !field.text.is_empty() && field.tracks.is_empty()
}

/// Draws a row of the Commands scope: the label, the menu it belongs to
/// and its keys (R5), dimmed while it can't run (R7). It says whether the
/// row was clicked.
fn command_row(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    index: usize,
    row: &CommandRow,
    highlight: bool,
) -> bool {
    let colors = &theme.colors;
    let id = row_id(index);
    let response = ui.interact(rect, id, Sense::click());
    if highlight {
        ui.painter().rect_filled(rect, 0.0, colors.control_hover.0);
    }
    let (ink, dim) = if row.available {
        (colors.text.0, colors.text_dim.0)
    } else {
        (colors.text_dim.0, colors.inactive.0)
    };
    let keys = keys_text(&row.keys);
    let painter = ui.painter_at(rect);
    let keys_width = painter
        .layout_no_wrap(keys.clone(), theme.small_font(), dim)
        .size()
        .x;
    painter.text(
        pos2(rect.right() - MARGIN - 2.0, rect.center().y),
        Align2::RIGHT_CENTER,
        &keys,
        theme.small_font(),
        dim,
    );
    let menu_right = rect.right() - MARGIN - keys_width - if keys.is_empty() { 0.0 } else { 12.0 };
    let menu_width = painter
        .layout_no_wrap(row.menu.clone(), theme.small_font(), dim)
        .size()
        .x
        .min(rect.width() / 3.0);
    let menu = Rect::from_min_max(
        pos2(menu_right - menu_width, rect.top()),
        pos2(menu_right, rect.bottom()),
    );
    ui.painter_at(menu).text(
        pos2(menu.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        &row.menu,
        theme.small_font(),
        dim,
    );
    let label = Rect::from_min_max(
        pos2(rect.left() + MARGIN + 2.0, rect.top()),
        pos2(menu.left() - 8.0, rect.bottom()),
    );
    ui.painter_at(label).text(
        pos2(label.left(), rect.center().y),
        Align2::LEFT_CENTER,
        &row.label,
        theme.font(),
        ink,
    );
    let described = if keys.is_empty() {
        format!("{}, {}", row.label, row.menu)
    } else {
        format!("{}, {}, {}", row.label, row.menu, keys)
    };
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::ListBoxOption);
        node.set_label(described);
        node.set_selected(highlight);
        node.set_bounds(bounds(rect));
        if !row.available {
            node.set_disabled();
        }
    });
    response.clicked()
}

/// Draws a row of the Tracks scope: the track's name and, when a device
/// matched, that device's name (R9). It says whether the row was clicked.
fn track_row(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    index: usize,
    row: &TrackRow,
    selected: bool,
) -> bool {
    let colors = &theme.colors;
    let id = row_id(index);
    let response = ui.interact(rect, id, Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 0.0, colors.lane_selected.0);
    }
    let painter = ui.painter_at(rect);
    painter.text(
        pos2(rect.left() + MARGIN + 2.0, rect.center().y),
        Align2::LEFT_CENTER,
        &row.name,
        theme.font(),
        colors.text.0,
    );
    if let Some(device) = &row.device {
        painter.text(
            pos2(rect.right() - MARGIN - 2.0, rect.center().y),
            Align2::RIGHT_CENTER,
            device,
            theme.small_font(),
            colors.text_dim.0,
        );
    }
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::ListBoxOption);
        node.set_label(match &row.device {
            Some(device) => format!("{}, {device}", row.name),
            None => row.name.clone(),
        });
        node.set_selected(selected);
        node.set_bounds(bounds(rect));
    });
    response.clicked()
}

/// The keys a row shows: the first binding, as the menus draw it.
fn keys_text(keys: &[String]) -> String {
    let platform = Platform::current();
    keys.iter()
        .filter_map(|binding| parse_binding(binding, platform))
        .filter_map(|chords| chords.first().map(|chord| chord_text(chord, platform)))
        .next()
        .or_else(|| keys.first().cloned())
        .unwrap_or_default()
}

/// What the Info View shows while a row of the Commands scope is
/// highlighted (R5): the row's label, its command's description and its
/// keys.
#[must_use]
pub fn info(shell: &Shell) -> Option<InfoViewContent> {
    let field = shell.search();
    if !field.open || field.scope != SearchScope::Commands {
        return None;
    }
    let row = field.commands.get(field.highlight)?;
    let description = shell.registry().get(&row.command)?.description.to_string();
    Some(InfoViewContent {
        title: row.label.clone(),
        body: description,
        shortcut: Some(keys_text(&row.keys)).filter(|keys| !keys.is_empty()),
    })
}

/// Outlines a track's title bar in the search colour while the field finds
/// that track (R10). Call it with the rectangle of the title bar.
pub fn outline(ui: &Ui, shell: &Shell, theme: &Theme, track: bl_model::TrackId, bar: Rect) {
    if shell.search_outlines(track) {
        ui.painter().rect_stroke(
            bar,
            0.0,
            Stroke::new(2.0, theme.colors.search.0),
            StrokeKind::Inside,
        );
    }
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
