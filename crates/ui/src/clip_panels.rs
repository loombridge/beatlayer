//! Where the Clip View puts its panels (WP-6.72; the detail views spec, R19
//! to R26): beside the editor or stacked in a column that scrolls, folded
//! away, the edge you drag to choose between the two, and the keys that
//! reach the panels.
//!
//! Every size here is in layout points, which the interface zoom scales as
//! it scales the whole window (interface-zoom R1).

use bl_model::ClipId;
use bl_shell::clip_properties_commands::{ARRANGE_PANELS, PanelArrangement};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::{self, Action, Role};
use egui::{
    Context, CursorIcon, EventFilter, Id, Key, Modifiers, Rect, Sense, Stroke, Ui, WidgetInfo,
    WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::clip_view::{TITLE_BAR_HEIGHT, panels_width, title_bar_rect};
use crate::launch_panel::panel_width;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How tall the left group is stacked: Main Properties' controls, the
/// taller of the two panels, fit in it (R19, Beatlayer's own).
pub const LEFT_HEIGHT: f32 = 212.0;
/// How tall the right group is stacked: the MIDI Generative Tools panel, the
/// tallest, fits in it (R19, Beatlayer's own).
pub const RIGHT_HEIGHT: f32 = 284.0;
/// How wide the panels' right edge is, inside the panels (I-DRAG-029).
const EDGE: f32 = 4.0;

/// Where the Clip View's parts sit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parts {
    /// The clip title bar (L-CLIP-002).
    pub title: Rect,
    /// The place the panels share, under the title bar: empty while folded.
    pub column: Rect,
    /// The left group, selector row and panel.
    pub left: Rect,
    /// The right group, selector row and panel.
    pub right: Rect,
    /// The editor.
    pub editor: Rect,
    /// The panels' right edge, which is dragged (I-DRAG-029): empty while
    /// folded.
    pub edge: Rect,
    /// How far the column can scroll: 0 unless the panels are stacked and
    /// taller than it.
    pub scrollable: f32,
}

/// Lays out a Clip View of `view`: the panels `vertical` or beside the
/// editor, `folded` away, and the stacked column scrolled `scroll` points.
#[must_use]
pub fn parts(view: Rect, vertical: bool, folded: bool, scroll: f32) -> Parts {
    if folded {
        // The title bar stays across the top and the editor takes the whole
        // width under it (R23).
        let title = title_bar_rect(view, view.width());
        return Parts {
            title,
            column: Rect::NOTHING,
            left: Rect::NOTHING,
            right: Rect::NOTHING,
            editor: Rect::from_min_max(pos2(view.left(), title.bottom()), view.max),
            edge: Rect::NOTHING,
            scrollable: 0.0,
        };
    }
    let wide = if vertical {
        panel_width(view)
    } else {
        panels_width(view)
    };
    let column = Rect::from_min_max(
        pos2(view.left(), view.top() + TITLE_BAR_HEIGHT),
        pos2(view.left() + wide, view.bottom()),
    );
    let edge = Rect::from_min_max(
        pos2(column.right() - EDGE, column.top()),
        pos2(column.right(), column.bottom()),
    );
    let editor = Rect::from_min_max(pos2(column.right(), view.top()), view.max);
    let title = title_bar_rect(view, wide);
    if vertical {
        let scrollable = (LEFT_HEIGHT + RIGHT_HEIGHT - column.height()).max(0.0);
        let top = column.top() - scroll.clamp(0.0, scrollable);
        let left = Rect::from_min_size(pos2(column.left(), top), vec2(wide, LEFT_HEIGHT));
        let right = Rect::from_min_size(left.left_bottom(), vec2(wide, RIGHT_HEIGHT));
        return Parts {
            title,
            column,
            left,
            right,
            editor,
            edge,
            scrollable,
        };
    }
    let left = Rect::from_min_size(column.min, vec2(panel_width(view), column.height()));
    let right = crate::tool_panel::panel_rect(column, left.right());
    Parts {
        title,
        column,
        left,
        right,
        editor,
        edge,
        scrollable: 0.0,
    }
}

/// Whether the Clip View of `view` arranges its panels vertically now (R20).
#[must_use]
pub fn is_vertical(shell: &Shell, view: Rect) -> bool {
    shell
        .clip_view()
        .panel_arrangement()
        .is_vertical(view.height())
}

fn scroll_id() -> Id {
    Id::new("clip-panels-scroll")
}

fn selected_request_id() -> Id {
    Id::new("clip-panels-selected-request")
}

fn last_group_id() -> Id {
    Id::new("clip-panels-last-group")
}

fn drag_id() -> Id {
    Id::new("clip-panels-edge-drag")
}

/// Asks for the keyboard focus to go to the first control of the shown panel
/// of the group the focus was last in (Option+8, R24).
pub fn request_selected_focus(ctx: &Context) {
    ctx.data_mut(|d| d.insert_temp(selected_request_id(), true));
}

/// A group of the panels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Group {
    #[default]
    Left,
    Right,
}

/// Draws the Clip View's title bar and panels for `clip`, and returns where
/// the editor goes. An `audio` clip's right group is Audio Utilities.
pub fn show(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    view: Rect,
    clip: ClipId,
    audio: bool,
    events: &mut Vec<UiEvent>,
) -> Rect {
    let ctx = ui.ctx().clone();
    let vertical = is_vertical(shell, view);
    let folded = shell.clip_view().panels_folded();
    let scroll = ctx.data(|d| d.get_temp::<f32>(scroll_id())).unwrap_or(0.0);
    let mut at = parts(view, vertical, folded, scroll);
    crate::clip_view::title_bar(ui, shell, theme, at.title, clip, audio, events);
    if folded {
        return at.editor;
    }
    // Spec: detail-views R19. The wheel scrolls a stacked column that is
    // taller than the Clip View.
    if at.scrollable > 0.0 && ui.rect_contains_pointer(at.column) {
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if wheel != 0.0 {
            let moved = (scroll - wheel).clamp(0.0, at.scrollable);
            ctx.data_mut(|d| d.insert_temp(scroll_id(), moved));
            at = parts(view, vertical, folded, moved);
        }
    }
    let wanted = ctx
        .data_mut(|d| d.remove_temp::<bool>(selected_request_id()))
        .unwrap_or(false);
    let last = ctx
        .data(|d| d.get_temp::<Group>(last_group_id()))
        .unwrap_or_default();
    if wanted && last == Group::Left {
        crate::main_panel::request_focus(&ctx);
    }
    // A stacked column is cut off at the Clip View's edge; beside the
    // editor the panels draw as they always have.
    let saved = ui.clip_rect();
    if vertical {
        ui.set_clip_rect(saved.intersect(at.column));
    }
    let before = crate::focus::drawn(&ctx).entries().len();
    crate::main_panel::show_left_group(ui, shell, theme, at.left, clip, events);
    let middle = crate::focus::drawn(&ctx).entries().len();
    if audio {
        crate::audio_panel::show(ui, shell, theme, at.right, clip, events);
    } else {
        crate::tool_panel::show(ui, shell, theme, at.right, clip, events);
    }
    let order = crate::focus::drawn(&ctx);
    ui.set_clip_rect(saved);
    let focused = |range: std::ops::Range<usize>| {
        order.entries()[range]
            .iter()
            .any(|entry| ctx.memory(|m| m.has_focus(entry.id)))
    };
    // The group the focus is in is the one Option+8 goes back to.
    if focused(before..middle) {
        ctx.data_mut(|d| d.insert_temp(last_group_id(), Group::Left));
    } else if focused(middle..order.entries().len()) {
        ctx.data_mut(|d| d.insert_temp(last_group_id(), Group::Right));
    }
    if wanted && last == Group::Right {
        let skip = crate::tool_panel::button_count(audio);
        if let Some(entry) = order.entries().get(middle + skip) {
            ctx.memory_mut(|m| m.request_focus(entry.id));
            // The group may be scrolled out of the column.
            ctx.data_mut(|d| d.insert_temp(scroll_id(), at.scrollable));
        }
    }
    edge(ui, theme, &at, vertical, events);
    at.editor
}

/// The panels' right edge (R21, R25): dragging it left past half the right
/// group's width chooses Vertically, and back to the right chooses
/// Horizontally; it stays where it is during the drag, and a shorter drag
/// changes nothing.
fn edge(ui: &mut Ui, theme: &Theme, at: &Parts, vertical: bool, events: &mut Vec<UiEvent>) {
    let id = Id::new("clip-panels-edge");
    let response = ui
        .interact(at.edge, id, Sense::drag())
        .on_hover_cursor(CursorIcon::ResizeHorizontal);
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Splitter);
        node.set_label("Clip View Panels".to_owned());
        node.set_numeric_value(f64::from(u8::from(!vertical)));
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(1.0);
        node.set_description(
            "Increment arranges the panels horizontally, decrement vertically".to_owned(),
        );
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    });
    let choose = |arrangement: PanelArrangement| {
        UiEvent::Invoke(Invocation::new(
            ARRANGE_PANELS,
            json!({ "arrangement": arrangement }),
        ))
    };
    let focused = response.has_focus();
    let step = ui.input(|i| {
        let keys = if focused {
            i32::from(i.key_pressed(Key::ArrowRight)) - i32::from(i.key_pressed(Key::ArrowLeft))
        } else {
            0
        };
        i.num_accesskit_action_requests(id, Action::Increment) as i32
            - i.num_accesskit_action_requests(id, Action::Decrement) as i32
            + keys
    });
    if step > 0 && vertical {
        events.push(choose(PanelArrangement::Horizontal));
    } else if step < 0 && !vertical {
        events.push(choose(PanelArrangement::Vertical));
    }
    let ctx = ui.ctx().clone();
    if response.drag_started()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        ctx.data_mut(|d| d.insert_temp(drag_id(), (pointer.x, pointer.x)));
    }
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        ctx.data_mut(|d| {
            if let Some((from, _)) = d.get_temp::<(f32, f32)>(drag_id()) {
                d.insert_temp(drag_id(), (from, pointer.x));
            }
        });
    }
    if response.drag_stopped()
        && let Some((from, to)) = ctx.data_mut(|d| d.remove_temp::<(f32, f32)>(drag_id()))
    {
        let far = crate::tool_panel::WIDTH / 2.0;
        if vertical && to - from >= far {
            events.push(choose(PanelArrangement::Horizontal));
        } else if !vertical && from - to >= far {
            events.push(choose(PanelArrangement::Vertical));
        }
    }
    if response.hovered() || response.dragged() {
        ui.painter().line_segment(
            [at.edge.center_top(), at.edge.center_bottom()],
            Stroke::new(2.0, theme.colors.focus.0),
        );
    }
}

/// The ID of the left group's selector button `index`.
pub(crate) fn left_tab_id(index: usize) -> Id {
    Id::new(("launch-panel-selector", index))
}

/// The ID of the right group's selector button `index`.
pub(crate) fn right_tab_id(audio: bool, index: usize) -> Id {
    Id::new(("clip-view-panel-selector", audio, index))
}

/// Whether a selector row button has the keyboard, which then takes Space as
/// Enter (R24) and not the transport's play key.
#[must_use]
pub fn tab_has_focus(ctx: &Context) -> bool {
    let Some(focused) = ctx.memory(|m| m.focused()) else {
        return false;
    };
    (0..2).any(|index| focused == left_tab_id(index))
        || (0..3).any(|index| {
            focused == right_tab_id(false, index) || focused == right_tab_id(true, index)
        })
}

/// Left and Right move the focus between the buttons `ids` of a selector row
/// and stop at its ends; Enter and Space show the focused button's panel as
/// a click does (R24).
pub(crate) fn selector_keys(ui: &Ui, ids: &[Id]) {
    for (index, id) in ids.iter().enumerate() {
        if !ui.memory(|m| m.has_focus(*id)) {
            continue;
        }
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                *id,
                EventFilter {
                    horizontal_arrows: true,
                    ..Default::default()
                },
            );
        });
        let (left, right) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(Modifiers::NONE, Key::ArrowRight),
            )
        });
        let to = if left {
            index.checked_sub(1)
        } else if right {
            Some(index + 1).filter(|next| *next < ids.len())
        } else {
            None
        };
        if let Some(to) = to {
            ui.memory_mut(|m| m.request_focus(ids[to]));
        }
    }
}

/// Announces the selector row `rect` as a tab list named `label`.
pub(crate) fn tab_list(ui: &Ui, id: Id, rect: Rect, label: &str) {
    let list = ui.interact(rect, id, Sense::hover());
    ui.ctx().accesskit_node_builder(list.id, |node| {
        node.set_role(Role::TabList);
        node.set_label(label.to_owned());
        node.set_bounds(accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
}

/// Announces the selector row button `id` as a tab named `name`, selected
/// while `lit`.
pub(crate) fn tab(ui: &Ui, response: &egui::Response, name: &str, lit: bool) {
    response.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, lit, name));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Tab);
        node.set_selected(lit);
    });
}
