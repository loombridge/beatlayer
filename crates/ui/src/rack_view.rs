//! A rack in the Device View (WP-5.2; the racks spec, R18 to R26): the view
//! column of selectors down its left edge, then each view that's shown,
//! left to right: the Macro Controls' place (WP-5.4), the chain list, and
//! the selected chain's devices, which the rack's end bracket closes. A rack
//! inside a chain draws the same way, as a bracket inside its parent's.
//!
//! The chain list has a row per chain (activator, name, Solo and Hot-Swap,
//! Volume and Pan) and a drop area below. Every gesture runs a registered
//! command (`chain.*`, `rack.set_view`), so the keyboard, a screen reader
//! and an agent reach the same ones.

use bl_model::{ChainId, DeviceId, RackView};
use bl_shell::context_menus::{self, CHAIN_COLOUR_ROW};
use bl_shell::device_commands::{
    CHAIN_DROP_AREA_HEIGHT, CHAIN_ROW_HEIGHT, ChainRow, DEVICE_GAP, DeviceFrame,
    RACK_CHAIN_LIST_WIDTH, RACK_COLUMN_WIDTH, RACK_EMPTY_DEVICES_WIDTH, RACK_END_BRACKET_WIDTH,
    RACK_MACROS_WIDTH, RackFrame, TITLE_BAR_HEIGHT, frame_width,
};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, Color32, DragAndDrop, Id, Key, Modifiers, Rect, Sense, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::device_view::DeviceViewUi;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The size of the chain list's round and square buttons.
const BUTTON: f32 = 11.0;

/// The height of Volume and Pan's sliders.
const SLIDER_HEIGHT: f32 = 9.0;

/// How many swatches a row of a chain's colour palette has (racks spec, R26).
pub const PALETTE_COLUMNS: usize = 14;

/// A chain's row being dragged: the chains that move, in the list's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainDrag {
    /// The chains.
    pub chains: Vec<ChainId>,
}

/// Where the controls of one chain's row sit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChainRowRects {
    /// The chain.
    pub chain: ChainId,
    /// The whole row.
    pub row: Rect,
    /// The Chain Activator.
    pub activator: Rect,
    /// The chain's name.
    pub name: Rect,
    /// The Solo button.
    pub solo: Rect,
    /// The Hot-Swap button.
    pub hot_swap: Rect,
    /// The Volume slider; empty where the chain has none.
    pub volume: Rect,
    /// The Pan slider; empty where the chain has none.
    pub pan: Rect,
}

/// Where a rack's chain list and its rows sit.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainListRects {
    /// The whole list.
    pub area: Rect,
    /// A row per chain, those scrolled out of the list included.
    pub rows: Vec<ChainRowRects>,
    /// The drop area below the rows.
    pub drop_area: Rect,
}

/// Where each part of a rack sits in its frame.
#[derive(Clone, Debug, PartialEq)]
pub struct RackLayout {
    /// The view selectors.
    pub selectors: Vec<(RackView, Rect)>,
    /// The Macro Controls' place, while it's shown.
    pub macros: Option<Rect>,
    /// The chain list, while it's shown.
    pub chain_list: Option<ChainListRects>,
    /// The devices view, while it's shown.
    pub devices: Option<Rect>,
    /// The selected chain's devices.
    pub device_frames: Vec<(DeviceId, Rect)>,
    /// The end bracket, while the devices view is shown.
    pub bracket: Option<Rect>,
}

/// The names the view selectors read.
#[must_use]
pub fn selector_name(view: RackView) -> &'static str {
    match view {
        RackView::Macros => "Macro Controls View",
        RackView::ChainList => "Chain List View",
        RackView::Devices => "Devices View",
    }
}

const VIEWS: [RackView; 3] = [RackView::Macros, RackView::ChainList, RackView::Devices];

/// The ID of a rack's view selector.
#[must_use]
pub fn selector_id(rack: DeviceId, view: RackView) -> Id {
    Id::new(("rack-selector", rack, view))
}

/// The ID of a chain row's whole area.
#[must_use]
pub fn row_id(chain: ChainId) -> Id {
    Id::new(("chain-row", chain))
}

/// The ID of one of a chain row's controls: `activator`, `solo`,
/// `hot_swap`, `volume` or `pan`.
#[must_use]
pub fn row_control_id(chain: ChainId, control: &str) -> Id {
    Id::new(("chain-control", chain, control))
}

/// The ID of a rack's chain list drop area.
#[must_use]
pub fn drop_area_id(rack: DeviceId) -> Id {
    Id::new(("chain-drop-area", rack))
}

/// Where a rack's view selectors sit when it's a strip (every view hidden):
/// under the activator.
fn strip_selectors(rect: Rect) -> Vec<(RackView, Rect)> {
    let x = rect.center().x;
    let first = rect.top() + 2.0 + BUTTON + 6.0 + BUTTON / 2.0;
    VIEWS
        .iter()
        .enumerate()
        .map(|(n, view)| {
            let y = first + n as f32 * (BUTTON + 3.0);
            (
                *view,
                Rect::from_center_size(pos2(x, y), vec2(BUTTON, BUTTON)),
            )
        })
        .collect()
}

/// Where a rack's parts sit in its frame at `rect`, with its chain list
/// scrolled by `scroll` points.
#[must_use]
pub fn layout(rack: &RackFrame, rect: Rect, scroll: f32) -> RackLayout {
    if rack.is_strip() {
        return RackLayout {
            selectors: strip_selectors(rect),
            macros: None,
            chain_list: None,
            devices: None,
            device_frames: Vec::new(),
            bracket: None,
        };
    }
    let body = Rect::from_min_max(pos2(rect.left(), rect.top() + TITLE_BAR_HEIGHT), rect.max);
    let column = Rect::from_min_size(body.min, vec2(RACK_COLUMN_WIDTH, body.height()));
    let selectors = VIEWS
        .iter()
        .enumerate()
        .map(|(n, view)| {
            let centre = pos2(
                column.center().x,
                column.top() + 4.0 + BUTTON / 2.0 + n as f32 * (BUTTON + 3.0),
            );
            (*view, Rect::from_center_size(centre, vec2(BUTTON, BUTTON)))
        })
        .collect();
    let mut x = column.right();
    let mut part = |width: f32| {
        let area = Rect::from_min_max(pos2(x, body.top()), pos2(x + width, body.bottom()));
        x += width;
        area
    };
    let macros = rack.views.macros.then(|| part(RACK_MACROS_WIDTH));
    let chain_list = rack
        .views
        .chain_list
        .then(|| chain_list(rack, part(RACK_CHAIN_LIST_WIDTH), scroll));
    let mut devices = None;
    let mut device_frames = Vec::new();
    let mut bracket = None;
    if rack.views.devices {
        let width = bl_shell::device_commands::rack_devices_width(rack);
        let area = part(width);
        let (top, bottom) = (area.top() + DEVICE_GAP, area.bottom() - DEVICE_GAP);
        let mut at = area.left() + DEVICE_GAP;
        for device in &rack.devices {
            let w = frame_width(device);
            device_frames.push((
                device.id,
                Rect::from_min_max(pos2(at, top), pos2(at + w, bottom)),
            ));
            at += w + DEVICE_GAP;
        }
        bracket = Some(Rect::from_min_max(
            pos2(area.right() - RACK_END_BRACKET_WIDTH, area.top()),
            area.max,
        ));
        devices = Some(area);
    }
    RackLayout {
        selectors,
        macros,
        chain_list,
        devices,
        device_frames,
        bracket,
    }
}

fn chain_list(rack: &RackFrame, area: Rect, scroll: f32) -> ChainListRects {
    let mut y = area.top() + 2.0 - scroll;
    let rows = rack
        .chains
        .iter()
        .map(|chain| {
            let row = Rect::from_min_max(
                pos2(area.left() + 2.0, y),
                pos2(area.right() - 2.0, y + CHAIN_ROW_HEIGHT - 2.0),
            );
            y += CHAIN_ROW_HEIGHT;
            row_rects(chain, row)
        })
        .collect();
    ChainListRects {
        area,
        rows,
        drop_area: Rect::from_min_max(
            pos2(area.left() + 2.0, y + 2.0),
            pos2(area.right() - 2.0, y + 2.0 + CHAIN_DROP_AREA_HEIGHT),
        ),
    }
}

fn row_rects(chain: &ChainRow, row: Rect) -> ChainRowRects {
    let line = row.top() + 8.0;
    let button = |x: f32| Rect::from_center_size(pos2(x, line), vec2(BUTTON, BUTTON));
    let hot_swap = button(row.right() - 2.0 - BUTTON / 2.0);
    let solo = button(hot_swap.center().x - BUTTON - 3.0);
    let activator = button(row.left() + 6.0 + BUTTON / 2.0);
    let name = Rect::from_min_max(
        pos2(activator.right() + 4.0, row.top() + 2.0),
        pos2(solo.left() - 3.0, row.top() + 14.0),
    );
    let second = row.top() + 16.0;
    let (volume, pan) = if chain.has_mixer {
        let inner = Rect::from_min_max(
            pos2(row.left() + 4.0, second),
            pos2(row.right() - 4.0, second + SLIDER_HEIGHT),
        );
        let split = inner.left() + inner.width() * 0.58;
        (
            Rect::from_min_max(inner.min, pos2(split - 2.0, inner.bottom())),
            Rect::from_min_max(pos2(split + 2.0, inner.top()), inner.max),
        )
    } else {
        (Rect::NOTHING, Rect::NOTHING)
    };
    ChainRowRects {
        chain: chain.id,
        row,
        activator,
        name,
        solo,
        hot_swap,
        volume,
        pan,
    }
}

/// The value a slider at `rect` holds with the pointer at `x`, from 0 at its
/// left edge to 1 at its right.
fn slider_position(rect: Rect, x: f32) -> f64 {
    f64::from(((x - rect.left()) / rect.width()).clamp(0.0, 1.0))
}

/// Volume's gain at a place on its slider, and the place of a gain: the
/// track fader's scale (racks spec, R5).
#[must_use]
pub fn volume_at(position: f64) -> f64 {
    bl_model::fader::fader_gain(position)
}

/// Pan at a place on its slider: from −1 at the left edge to 1 at the
/// right.
#[must_use]
pub fn pan_at(position: f64) -> f64 {
    let pan = position * 2.0 - 1.0;
    // The centre is exact, whatever the pointer's pixel.
    if pan.abs() < 0.02 { 0.0 } else { pan }
}

/// The text Volume reads: its gain in decibels.
#[must_use]
pub fn volume_text(gain: f64) -> String {
    if gain <= 0.0 {
        "-inf dB".to_owned()
    } else {
        format!("{:.1} dB", bl_model::fader::gain_db(gain))
    }
}

/// The text Pan reads.
#[must_use]
pub fn pan_text(pan: f64) -> String {
    let percent = (pan.abs() * 50.0).round();
    if percent == 0.0 {
        "C".to_owned()
    } else if pan < 0.0 {
        format!("{percent}L")
    } else {
        format!("{percent}R")
    }
}

/// The colour of swatch `index` of a chain's palette: the theme's track
/// colours across, each row lighter or darker than the last (racks spec,
/// R26).
#[must_use]
pub fn swatch(theme: &Theme, index: u8) -> Color32 {
    let palette = &theme.colors.track_palette;
    let column = usize::from(index) % PALETTE_COLUMNS;
    let row = usize::from(index) / PALETTE_COLUMNS;
    let base = palette[column % palette.len()].0;
    let mix: f32 = [0.0, 0.3, 0.6, -0.3, -0.5][row.min(4)];
    let towards = if mix < 0.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    base.lerp_to_gamma(towards, mix.abs())
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// How a click on a chain row changes the selection (racks spec, R22):
/// Shift selects a range, Cmd (Ctrl) adds or removes one.
#[must_use]
pub fn click_mode(modifiers: Modifiers) -> &'static str {
    if modifiers.shift {
        "range"
    } else if modifiers.command {
        "toggle"
    } else {
        "replace"
    }
}

/// A chain's name being typed in its row.
pub(crate) struct ChainRenaming {
    pub chain: ChainId,
    pub text: String,
    pub focused: bool,
}

impl DeviceViewUi {
    /// Draws the body of `rack` in its frame at `rect`: the view column, and
    /// each view that's shown.
    #[expect(clippy::too_many_arguments, reason = "the body's inputs")]
    pub(crate) fn rack_body(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        rack: &RackFrame,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let scroll = self.chain_scroll(frame.id);
        let layout = layout(rack, rect, scroll);
        let painter = ui.painter_at(rect);
        if !rack.is_strip() {
            let body =
                Rect::from_min_max(pos2(rect.left(), rect.top() + TITLE_BAR_HEIGHT), rect.max);
            let column = Rect::from_min_size(body.min, vec2(RACK_COLUMN_WIDTH, body.height()));
            painter.rect_filled(column, 0.0, c.control.0.gamma_multiply(0.6));
        }
        for (view, at) in &layout.selectors {
            self.view_selector(ui, theme, frame, rack, *view, *at, events);
        }
        if let Some(area) = layout.macros {
            // The Macro Controls' place (WP-5.4 fills it).
            let id = Id::new(("rack-macros", frame.id));
            ui.interact(area, id, Sense::hover());
            painter.rect_filled(area.shrink(1.0), 0.0, c.window.0.gamma_multiply(0.5));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Group);
                node.set_label("Macro Controls".to_owned());
            });
        }
        if let Some(list) = &layout.chain_list {
            self.chain_list(ui, shell, theme, frame, rack, list, events);
        }
        if let Some(area) = layout.devices {
            painter.rect_filled(area, 0.0, c.panel.0.gamma_multiply(0.7));
            if rack.devices.is_empty() {
                let text = rack.note.as_deref().unwrap_or(if rack.chains.is_empty() {
                    "No chains"
                } else {
                    "Drop a device here"
                });
                let text_area =
                    Rect::from_min_size(area.min, vec2(RACK_EMPTY_DEVICES_WIDTH, area.height()));
                painter.text(
                    text_area.center(),
                    Align2::CENTER_CENTER,
                    text,
                    theme.small_font(),
                    c.text_dim.0,
                );
                let id = Id::new(("rack-devices-note", frame.id));
                ui.interact(text_area, id, Sense::hover());
                ui.ctx().accesskit_node_builder(id, |node| {
                    node.set_role(Role::Label);
                    node.set_label(text.to_owned());
                });
            }
        }
        for (child, at) in rack.devices.iter().zip(&layout.device_frames) {
            debug_assert_eq!(child.id, at.0);
            self.device(ui, shell, theme, child, at.1, false, events);
        }
        if let Some(bracket) = layout.bracket {
            // The end bracket closes the devices (racks spec, R18).
            let stroke = Stroke::new(1.5, c.text_dim.0);
            let x = bracket.left() + 2.0;
            let (top, bottom) = (bracket.top() + 2.0, bracket.bottom() - 2.0);
            painter.line_segment([pos2(x, top), pos2(x, bottom)], stroke);
            painter.line_segment([pos2(x, top), pos2(x + 4.0, top)], stroke);
            painter.line_segment([pos2(x, bottom), pos2(x + 4.0, bottom)], stroke);
        }
    }

    /// One of the view column's selectors.
    #[expect(clippy::too_many_arguments, reason = "the selector's inputs")]
    fn view_selector(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        rack: &RackFrame,
        view: RackView,
        at: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let id = selector_id(frame.id, view);
        let response = ui.interact(at, id, Sense::click());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let on = rack.views.shown(view);
        if response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
            invoke(
                events,
                "rack.set_view",
                json!({ "rack": frame.id, "view": view, "shown": !on }),
            );
        }
        let c = &theme.colors;
        let face = if on {
            c.accent.0
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.window.0
        };
        let painter = ui.painter_at(at.expand(2.0));
        painter.rect_filled(at, theme.sizes.corner, face);
        let ink = if on { c.accent_text.0 } else { c.text_dim.0 };
        let stroke = Stroke::new(1.0, ink);
        let c0 = at.center();
        match view {
            // Knobs.
            RackView::Macros => {
                painter.circle_stroke(c0, 3.0, stroke);
            }
            // Rows.
            RackView::ChainList => {
                for dy in [-2.5, 0.0, 2.5] {
                    painter.line_segment(
                        [pos2(c0.x - 3.0, c0.y + dy), pos2(c0.x + 3.0, c0.y + dy)],
                        stroke,
                    );
                }
            }
            // A device.
            RackView::Devices => {
                painter.rect_stroke(
                    Rect::from_center_size(c0, vec2(6.0, 6.0)),
                    0.5,
                    stroke,
                    StrokeKind::Inside,
                );
            }
        }
        if response.has_focus() && crate::focus::outlined(ui.ctx()) {
            painter.rect_stroke(
                at.expand(1.5),
                theme.sizes.corner,
                Stroke::new(1.0, c.focus.0),
                StrokeKind::Outside,
            );
        }
        let name = selector_name(view);
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(name.to_owned());
            node.set_value(if on { "shown" } else { "hidden" }.to_owned());
            node.set_description(format!(
                "Shows or hides the rack's {}",
                name.trim_end_matches(" View").to_lowercase()
            ));
        });
    }

    /// The scroll of a rack's chain list, in points.
    fn chain_scroll(&self, rack: DeviceId) -> f32 {
        self.chain_scrolls.get(&rack).copied().unwrap_or(0.0)
    }

    /// The chain list: a row per chain, and the drop area below.
    #[expect(clippy::too_many_arguments, reason = "the list's inputs")]
    fn chain_list(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        rack: &RackFrame,
        list: &ChainListRects,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let list_id = Id::new(("chain-list", frame.id));
        let area_response = ui.interact(list.area, list_id, Sense::hover());
        ui.ctx().accesskit_node_builder(list_id, |node| {
            node.set_role(Role::List);
            node.set_label(format!("Chain List of {}", frame.title));
        });
        let painter = ui.painter_at(list.area);
        painter.rect_filled(list.area, 0.0, c.window.0.gamma_multiply(0.5));
        // The wheel scrolls a list taller than the rack.
        let content = list.rows.len() as f32 * CHAIN_ROW_HEIGHT + CHAIN_DROP_AREA_HEIGHT + 6.0;
        if area_response.hovered() || ui.rect_contains_pointer(list.area) {
            let delta = ui.input(|i| i.smooth_scroll_delta.y);
            if delta != 0.0 {
                let max = (content - list.area.height()).max(0.0);
                let now = (self.chain_scroll(frame.id) - delta).clamp(0.0, max);
                self.chain_scrolls.insert(frame.id, now);
            }
        }
        self.chain_keys(ui, shell, rack, events);
        let drag = DragAndDrop::payload::<ChainDrag>(ui.ctx());
        for (row, rects) in rack.chains.iter().zip(&list.rows) {
            if rects.row.bottom() < list.area.top() || rects.row.top() > list.area.bottom() {
                continue;
            }
            self.chain_row(ui, shell, theme, rack, row, rects, list, events);
        }
        self.chain_drop(ui, theme, frame, rack, list, drag.as_deref(), events);
    }

    /// Up and Down step the selection while the chain list has the focus
    /// (racks spec, R22).
    fn chain_keys(&self, ui: &Ui, shell: &Shell, rack: &RackFrame, events: &mut Vec<UiEvent>) {
        if !rack.chain_list_focused
            || shell.focused_area() != Area::Detail
            || self.renaming_chain.is_some()
            || egui::Popup::is_any_open(ui.ctx())
        {
            return;
        }
        // A row, or nothing, has the keyboard: not a slider, which steps
        // itself with these keys.
        let on_a_row = ui.memory(|m| {
            m.focused()
                .is_none_or(|id| rack.chains.iter().any(|c| row_id(c.id) == id))
        });
        if !on_a_row {
            return;
        }
        for (key, mode) in [(Key::ArrowUp, "previous"), (Key::ArrowDown, "next")] {
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, key)) {
                invoke(events, "chain.select", json!({ "mode": mode }));
            }
        }
    }

    /// One chain's row.
    #[expect(clippy::too_many_arguments, reason = "the row's inputs")]
    fn chain_row(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rack: &RackFrame,
        row: &ChainRow,
        rects: &ChainRowRects,
        list: &ChainListRects,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let clip = list.area;
        let painter = ui.painter_at(clip);
        let id = row_id(row.id);
        let response = ui.interact(rects.row.intersect(clip), id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let face = if row.selected {
            if rack.chain_list_focused {
                c.accent.0.gamma_multiply(0.55)
            } else {
                c.control_hover.0
            }
        } else if response.hovered() {
            c.control_hover.0.gamma_multiply(0.6)
        } else {
            c.control.0
        };
        painter.rect_filled(rects.row, theme.sizes.corner, face);
        if let Some(colour) = row.colour {
            let stripe = Rect::from_min_size(rects.row.min, vec2(3.0, rects.row.height()));
            painter.rect_filled(stripe, 0.0, swatch(theme, colour));
        }
        let entered = response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if response.clicked() || response.secondary_clicked() || entered {
            let modifiers = ui.input(|i| i.modifiers);
            let mode = if response.secondary_clicked() && row.selected {
                None
            } else if response.secondary_clicked() {
                Some("replace")
            } else {
                Some(click_mode(modifiers))
            };
            if let Some(mode) = mode {
                invoke(
                    events,
                    "chain.select",
                    json!({ "chain": row.id, "mode": mode }),
                );
            }
        }
        if response.drag_started() {
            let chains: Vec<ChainId> = if row.selected {
                rack.selected_chains.clone()
            } else {
                invoke(
                    events,
                    "chain.select",
                    json!({ "chain": row.id, "mode": "replace" }),
                );
                vec![row.id]
            };
            DragAndDrop::set_payload(ui.ctx(), ChainDrag { chains });
        }
        response.context_menu(|ui| self.chain_menu(ui, shell, theme, row, events));
        // Shift-F10 (S-CTX-002) opens the same menu from the keyboard.
        if response.has_focus() && crate::menus::context_menu_keys(ui) {
            invoke(
                events,
                "chain.select",
                json!({ "chain": row.id, "mode": "replace" }),
            );
            crate::menus::open_keyboard_context_menu(
                ui.ctx(),
                response.id,
                rects.row.left_bottom(),
            );
        }
        crate::menus::keyboard_context_menu(&response, |ui| {
            self.chain_menu(ui, shell, theme, row, events);
        });
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, true, row.selected, row.name.as_str())
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(format!("Chain {}", row.name));
            node.set_selected(row.selected);
            node.set_description(
                "Chain row: click to select, Shift-click for a range, Cmd-click to add, drag to \
                 move"
                    .to_owned(),
            );
        });
        if rack.chain_list_focused && row.selected && response.has_focus() {
            painter.rect_stroke(
                rects.row,
                theme.sizes.corner,
                Stroke::new(1.0, c.focus.0),
                StrokeKind::Inside,
            );
        }
        self.chain_name(ui, theme, row, rects, &painter, events);
        self.chain_activator(ui, theme, row, rects, events);
        self.chain_toggle(ui, theme, row, rects.solo, "solo", row.solo, events);
        self.chain_toggle(
            ui,
            theme,
            row,
            rects.hot_swap,
            "hot_swap",
            row.hot_swap,
            events,
        );
        if row.has_mixer {
            self.chain_slider(ui, theme, row, rects.volume, "volume", events);
            self.chain_slider(ui, theme, row, rects.pan, "pan", events);
        }
    }

    /// A chain's name, or the text field it's being renamed in.
    fn chain_name(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        row: &ChainRow,
        rects: &ChainRowRects,
        painter: &egui::Painter,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        if let Some(renaming) = self.renaming_chain.as_mut().filter(|r| r.chain == row.id) {
            let edit = ui.put(
                rects.name,
                egui::TextEdit::singleline(&mut renaming.text)
                    .id(row_control_id(row.id, "name"))
                    .font(theme.small_font())
                    .margin(vec2(2.0, 0.0)),
            );
            if !renaming.focused {
                edit.request_focus();
                renaming.focused = true;
            }
            let (enter, escape) =
                ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if edit.lost_focus() || enter || escape {
                if enter || (edit.lost_focus() && !escape) {
                    invoke(
                        events,
                        "chain.rename",
                        json!({ "chain": row.id, "name": renaming.text }),
                    );
                }
                self.renaming_chain = None;
            }
            return;
        }
        let ink = if row.enabled { c.text.0 } else { c.text_dim.0 };
        painter.text(
            pos2(rects.name.left(), rects.name.center().y),
            Align2::LEFT_CENTER,
            &row.name,
            theme.small_font(),
            ink,
        );
    }

    /// A chain's Chain Activator.
    fn chain_activator(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        row: &ChainRow,
        rects: &ChainRowRects,
        events: &mut Vec<UiEvent>,
    ) {
        let id = row_control_id(row.id, "activator");
        let response = ui.interact(rects.activator, id, Sense::click());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        if response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
            invoke(
                events,
                "chain.set_enabled",
                json!({ "chain": row.id, "enabled": !row.enabled }),
            );
        }
        let c = &theme.colors;
        let painter = ui.painter_at(rects.activator.expand(2.0));
        let r = rects.activator.width() / 2.0;
        painter.circle_filled(
            rects.activator.center(),
            r,
            if row.enabled { c.accent.0 } else { c.window.0 },
        );
        painter.circle_stroke(rects.activator.center(), r, Stroke::new(1.0, c.separator.0));
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, true, row.enabled, "Chain Activator")
        });
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(format!("Chain Activator of {}", row.name));
            node.set_value(if row.enabled { "on" } else { "off" }.to_owned());
        });
    }

    /// A chain's Solo or Hot-Swap button.
    #[expect(clippy::too_many_arguments, reason = "the button's inputs")]
    fn chain_toggle(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        row: &ChainRow,
        at: Rect,
        which: &'static str,
        on: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let id = row_control_id(row.id, which);
        let response = ui.interact(at, id, Sense::click());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let pressed =
            response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
        if pressed {
            if which == "solo" {
                // Spec: racks R23. A click solos it alone; Cmd-click (Ctrl-click)
                // adds or removes it.
                let add = ui.input(|i| i.modifiers.command);
                invoke(events, "chain.solo", json!({ "chain": row.id, "add": add }));
            } else {
                invoke(events, "chain.hot_swap", json!({ "chain": row.id }));
            }
        }
        let c = &theme.colors;
        let painter = ui.painter_at(at.expand(2.0));
        let face = if on {
            c.accent.0
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.window.0
        };
        let ink = if on { c.accent_text.0 } else { c.text_dim.0 };
        if which == "solo" {
            painter.rect_filled(at, theme.sizes.corner, face);
            painter.text(
                at.center(),
                Align2::CENTER_CENTER,
                "S",
                theme.small_font(),
                ink,
            );
        } else {
            painter.circle_filled(at.center(), at.width() / 2.0, face);
            painter.circle_stroke(at.center(), at.width() / 4.0, Stroke::new(1.0, ink));
        }
        let name = if which == "solo" { "Solo" } else { "Hot-Swap" };
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(format!("{name} of {}", row.name));
            node.set_value(if on { "on" } else { "off" }.to_owned());
        });
    }

    /// A chain's Volume or Pan: a drag sets it where the pointer is, one undo
    /// step; Up and Down step it, and a double-click or Delete resets it.
    fn chain_slider(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        row: &ChainRow,
        at: Rect,
        which: &'static str,
        events: &mut Vec<UiEvent>,
    ) {
        let id = row_control_id(row.id, which);
        let response = ui.interact(at, id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let (command, key, default) = if which == "volume" {
            ("chain.set_gain", "volume", 1.0)
        } else {
            ("chain.set_pan", "pan", 0.0)
        };
        let name = slider_name(which);
        let set = |value: f64| Invocation::new(command, json!({ "chain": row.id, key: value }));
        let at_pointer = |x: f32| {
            let position = slider_position(at, x);
            set(if which == "volume" {
                volume_at(position)
            } else {
                pan_at(position)
            })
        };
        // One undo step for a drag, a click, a reset or a key.
        let step = |events: &mut Vec<UiEvent>, invocation: Invocation| {
            events.push(UiEvent::BeginNamedGesture(format!("Change \"{name}\"")));
            events.push(UiEvent::Invoke(invocation));
            events.push(UiEvent::EndGesture);
        };
        // While it has the keyboard, Delete resets it rather than deleting
        // the selected chains (S-DEV-006).
        let focused = response.has_focus();
        let was = ui
            .data(|d| d.get_temp::<bool>(id.with("focused")))
            .unwrap_or(false);
        if focused != was {
            ui.data_mut(|d| d.insert_temp(id.with("focused"), focused));
            events.push(UiEvent::FocusValue(focused));
        }
        let (steps, reset) = Self::slider_keys(ui, &response);
        if reset || response.double_clicked() {
            step(events, set(default));
        } else if steps != 0 {
            let value = if which == "volume" {
                let position =
                    bl_model::fader::fader_position(row.volume) + f64::from(steps) * 0.02;
                volume_at(position.clamp(0.0, 1.0))
            } else {
                (row.pan + f64::from(steps) * 0.05).clamp(-1.0, 1.0)
            };
            step(events, set(value));
        } else if response.drag_started() {
            events.push(UiEvent::BeginNamedGesture(format!("Change \"{name}\"")));
        }
        if response.dragged()
            && !response.double_clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            events.push(UiEvent::Invoke(at_pointer(pointer.x)));
        }
        if response.drag_stopped() {
            events.push(UiEvent::EndGesture);
        } else if response.clicked()
            && !response.double_clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            step(events, at_pointer(pointer.x));
        }
        let c = &theme.colors;
        let painter = ui.painter_at(at.expand(1.0));
        painter.rect_filled(at, theme.sizes.corner, c.window.0);
        let (value, text) = if which == "volume" {
            (
                bl_model::fader::fader_position(row.volume) as f32,
                volume_text(row.volume),
            )
        } else {
            (((row.pan + 1.0) / 2.0) as f32, pan_text(row.pan))
        };
        let fill = if which == "volume" {
            Rect::from_min_max(at.min, pos2(at.left() + at.width() * value, at.bottom()))
        } else {
            let centre = at.center().x;
            let x = at.left() + at.width() * value;
            Rect::from_x_y_ranges(centre.min(x)..=centre.max(x), at.y_range())
        };
        painter.rect_filled(fill, theme.sizes.corner, c.accent.0.gamma_multiply(0.8));
        painter.text(
            at.center(),
            Align2::CENTER_CENTER,
            &text,
            egui::FontId::proportional(8.0),
            c.text.0,
        );
        if response.has_focus() && crate::focus::outlined(ui.ctx()) {
            painter.rect_stroke(
                at.expand(1.0),
                theme.sizes.corner,
                Stroke::new(1.0, c.focus.0),
                StrokeKind::Outside,
            );
        }
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Slider, true, name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Slider);
            node.add_action(egui::accesskit::Action::Increment);
            node.add_action(egui::accesskit::Action::Decrement);
            node.set_label(format!("Chain {name} of {}", row.name));
            node.set_value(text.clone());
            node.set_description(
                "Drag to set it, Up and Down step it, double-click or Delete reset it".to_owned(),
            );
        });
    }

    /// The steps Up and Down (or a screen reader's Increment and Decrement)
    /// move a focused slider, and whether Delete resets it.
    fn slider_keys(ui: &Ui, response: &egui::Response) -> (i32, bool) {
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        let mut steps = ui.input(|i| {
            i.num_accesskit_action_requests(response.id, egui::accesskit::Action::Increment) as i32
                - i.num_accesskit_action_requests(response.id, egui::accesskit::Action::Decrement)
                    as i32
        });
        let mut reset = false;
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        escape: true,
                        ..Default::default()
                    },
                );
            });
            ui.input(|i| {
                for event in &i.events {
                    if let egui::Event::Key {
                        key, pressed: true, ..
                    } = event
                    {
                        match key {
                            Key::ArrowUp => steps += 1,
                            Key::ArrowDown => steps -= 1,
                            Key::Delete => reset = true,
                            _ => {}
                        }
                    }
                }
            });
        }
        (steps, reset)
    }

    /// The drop area below the chain list's rows, and where dragged chains,
    /// devices and presets land (racks spec, R21, R24).
    #[expect(clippy::too_many_arguments, reason = "the drop's inputs")]
    fn chain_drop(
        &self,
        ui: &Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        rack: &RackFrame,
        list: &ChainListRects,
        chains: Option<&ChainDrag>,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let id = drop_area_id(frame.id);
        ui.interact(list.drop_area.intersect(list.area), id, Sense::hover());
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Group);
            node.set_label("Chain List Drop Area".to_owned());
            node.set_description("Drop a device, a preset or a chain here to add it".to_owned());
        });
        let painter = ui.painter_at(list.area);
        painter.rect_stroke(
            list.drop_area,
            theme.sizes.corner,
            Stroke::new(1.0, c.separator.0),
            StrokeKind::Inside,
        );
        painter.text(
            list.drop_area.center(),
            Align2::CENTER_CENTER,
            "Drop here",
            egui::FontId::proportional(9.0),
            c.text_dim.0,
        );
        let (pointer, released, dropped) = ui.input(|i| {
            let dropped: Vec<String> = i
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_string_lossy().into_owned())
                .collect();
            (i.pointer.interact_pos(), i.pointer.any_released(), dropped)
        });
        let Some(pointer) = pointer else {
            return;
        };
        let over_list = list.area.contains(pointer);
        // A preset dropped from the desktop makes a chain holding a device
        // made from it.
        if over_list && let Some(path) = dropped.iter().find(|p| p.ends_with(".blpreset")) {
            invoke(
                events,
                "chain.create",
                json!({ "rack": frame.id, "preset": path }),
            );
            return;
        }
        if !over_list {
            return;
        }
        // Spec: racks R43, R46. Presets and devices from the browser land
        // as chains, where the rows would take a moved chain.
        let browser = DragAndDrop::payload::<crate::browser::DevicesDrag>(ui.ctx());
        // Where among the rows it would land: the gap nearest the pointer.
        let gap = list
            .rows
            .iter()
            .filter(|r| r.row.center().y < pointer.y)
            .count();
        if chains.is_some() || browser.is_some() {
            let y = match list.rows.get(gap) {
                Some(next) => next.row.top() - 1.0,
                None => list
                    .rows
                    .last()
                    .map_or(list.area.top() + 2.0, |r| r.row.bottom() + 1.0),
            };
            painter.line_segment(
                [
                    pos2(list.area.left() + 2.0, y),
                    pos2(list.area.right() - 2.0, y),
                ],
                Stroke::new(2.0, c.accent.0),
            );
            if released && let Some(drag) = browser {
                DragAndDrop::clear_payload(ui.ctx());
                for (n, path) in drag.paths.iter().enumerate() {
                    let at = gap + n;
                    match bl_shell::browser::device_by_id(path) {
                        Some(device) => invoke(
                            events,
                            "chain.create",
                            json!({
                                "rack": frame.id,
                                "index": at,
                                "device": {
                                    "kind": device.kind,
                                    "name": device.name,
                                    "role": device.role,
                                    "sidechain": device.sidechain,
                                    "midi_sidechain": device.midi_sidechain,
                                },
                            }),
                        ),
                        None => invoke(
                            events,
                            "chain.create",
                            json!({ "rack": frame.id, "index": at, "preset": path }),
                        ),
                    }
                }
            }
        }
        if let Some(drag) = chains {
            if released {
                DragAndDrop::clear_payload(ui.ctx());
                let before = rack
                    .chains
                    .iter()
                    .take(gap)
                    .filter(|chain| !drag.chains.contains(&chain.id))
                    .count();
                events.push(UiEvent::BeginNamedGesture("Move Chain".to_owned()));
                // The rack's own dragged chains go to the end first, so each
                // move below lands among chains that stay put.
                let last = rack.chains.len().saturating_sub(1);
                for chain in drag
                    .chains
                    .iter()
                    .filter(|c| rack.chains.iter().any(|r| r.id == **c))
                {
                    invoke(
                        events,
                        "chain.move",
                        json!({ "chain": chain, "rack": frame.id, "index": last }),
                    );
                }
                for (n, chain) in drag.chains.iter().enumerate() {
                    invoke(
                        events,
                        "chain.move",
                        json!({ "chain": chain, "rack": frame.id, "index": before + n }),
                    );
                }
                events.push(UiEvent::EndGesture);
            }
        } else if let Some(device) =
            DragAndDrop::payload::<crate::device_view::DeviceDrag>(ui.ctx())
            && list.drop_area.contains(pointer)
        {
            painter.rect_stroke(
                list.drop_area,
                theme.sizes.corner,
                Stroke::new(2.0, c.accent.0),
                StrokeKind::Inside,
            );
            if released {
                DragAndDrop::clear_payload(ui.ctx());
                invoke(
                    events,
                    "chain.create",
                    json!({ "rack": frame.id, "move_device": device.device }),
                );
            }
        }
    }

    /// A chain's context menu (racks spec, R26): Rename, Edit Info Text, and
    /// the colour palette.
    fn chain_menu(
        &self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        row: &ChainRow,
        events: &mut Vec<UiEvent>,
    ) {
        let mut contexts = vec![bl_commands::Context::RackChains];
        contexts.extend(shell.active_contexts());
        for item in bl_shell::menus::shown(&self.chain_menu) {
            if item.is_separator() {
                ui.separator();
                continue;
            }
            if item.row == CHAIN_COLOUR_ROW {
                ui.label("Colour");
                self.chain_palette(ui, theme, row, events);
                continue;
            }
            let response = crate::menus::item_in(
                ui,
                item,
                shell,
                &self.keymap,
                self.platform,
                &contexts,
                events,
            );
            let response = match context_menus::pending_package(item.row) {
                Some(package) => {
                    let text = format!("Not built yet: arrives with {package}.");
                    ui.ctx().accesskit_node_builder(response.id, |node| {
                        node.set_description(text.clone());
                    });
                    response.on_disabled_hover_text(text)
                }
                None => response,
            };
            crate::menus::take_focus_first(ui, &response);
        }
    }

    /// The colour palette: 5 rows of 14 swatches, the chain's ticked.
    fn chain_palette(&self, ui: &mut Ui, theme: &Theme, row: &ChainRow, events: &mut Vec<UiEvent>) {
        let size = 12.0;
        let (area, _) = ui.allocate_exact_size(
            vec2(PALETTE_COLUMNS as f32 * (size + 2.0), 5.0 * (size + 2.0)),
            Sense::hover(),
        );
        for index in 0..bl_model::CHAIN_COLOURS {
            let (column, line) = (
                usize::from(index) % PALETTE_COLUMNS,
                usize::from(index) / PALETTE_COLUMNS,
            );
            let at = Rect::from_min_size(
                area.min + vec2(column as f32 * (size + 2.0), line as f32 * (size + 2.0)),
                vec2(size, size),
            );
            let id = Id::new(("chain-colour", row.id, index));
            let response = ui.interact(at, id, Sense::click());
            ui.painter().rect_filled(at, 2.0, swatch(theme, index));
            if row.colour == Some(index) {
                ui.painter().rect_stroke(
                    at,
                    2.0,
                    Stroke::new(1.5, theme.colors.text.0),
                    StrokeKind::Inside,
                );
            }
            response.widget_info(|| {
                WidgetInfo::selected(
                    WidgetType::Button,
                    true,
                    row.colour == Some(index),
                    format!("Colour {}", index + 1),
                )
            });
            if response.clicked() {
                invoke(
                    events,
                    "chain.set_colour",
                    json!({ "chain": row.id, "colour": index }),
                );
                ui.close();
            }
        }
    }
}

/// What a chain's slider is called.
fn slider_name(which: &str) -> &'static str {
    if which == "volume" { "Volume" } else { "Pan" }
}

/// Whether a chain is being dragged, for the track a pointer hovers to take
/// the focus (racks spec, R24).
#[must_use]
pub fn dragging_chain(ctx: &egui::Context) -> bool {
    DragAndDrop::payload::<ChainDrag>(ctx).is_some()
}

/// While a chain is dragged, the track whose title bar `bar` is and the
/// pointer is over takes the focus, so its Device View shows and the chain
/// can be dropped into a rack there (racks spec, R24).
pub fn focus_track_under_drag(
    ui: &Ui,
    shell: &Shell,
    track: bl_model::TrackId,
    bar: Rect,
    events: &mut Vec<UiEvent>,
) {
    if dragging_chain(ui.ctx())
        && ui.rect_contains_pointer(bar)
        && shell.arrangement().selected_tracks().first() != Some(&track)
    {
        invoke(
            events,
            "arrangement.select_tracks",
            json!({ "tracks": [track] }),
        );
    }
}

/// Where every device's frame sits in a Device View at `rect` scrolled by
/// `scroll` points, the devices inside racks included: a rack's selected
/// chain's devices follow the rack.
#[must_use]
pub fn all_frames(
    state: &bl_shell::device_commands::DeviceViewState,
    rect: Rect,
    scroll: f32,
) -> Vec<(DeviceId, Rect)> {
    fn walk(device: &DeviceFrame, at: Rect, found: &mut Vec<(DeviceId, Rect)>) {
        found.push((device.id, at));
        if let Some(rack) = &device.rack {
            let here = layout(rack, at, 0.0);
            for (child, (_, child_at)) in rack.devices.iter().zip(&here.device_frames) {
                walk(child, *child_at, found);
            }
        }
    }
    let mut found = Vec::new();
    for (device, (_, at)) in state
        .devices
        .iter()
        .zip(crate::device_view::frames(state, rect, scroll))
    {
        walk(device, at, &mut found);
    }
    found
}

/// Positions of every rack in the Device View at `rect`, nested ones
/// included, for tests and view captures: [`layout`] of each at the frame
/// [`crate::device_view::frames`] and its parents' layouts give it.
#[must_use]
pub fn layouts(shell: &Shell, rect: Rect) -> Vec<(DeviceId, RackLayout)> {
    let state = shell.device_view_state();
    let mut found = Vec::new();
    let frames = crate::device_view::frames(&state, rect, 0.0);
    for (device, (_, at)) in state.devices.iter().zip(frames) {
        collect(device, at, &mut found);
    }
    found
}

/// Whether `pointer` is over a rack's chain list in the Device View at
/// `rect`, scrolled by `scroll`: the list takes the wheel there, so the
/// Device View doesn't scroll as well (racks spec, R21).
#[must_use]
pub fn over_chain_list(
    state: &bl_shell::device_commands::DeviceViewState,
    rect: Rect,
    scroll: f32,
    pointer: egui::Pos2,
) -> bool {
    let mut found = Vec::new();
    for (device, (_, at)) in state
        .devices
        .iter()
        .zip(crate::device_view::frames(state, rect, scroll))
    {
        collect(device, at, &mut found);
    }
    found.iter().any(|(_, layout)| {
        layout
            .chain_list
            .as_ref()
            .is_some_and(|list| list.area.contains(pointer))
    })
}

fn collect(device: &DeviceFrame, at: Rect, found: &mut Vec<(DeviceId, RackLayout)>) {
    let Some(rack) = &device.rack else {
        return;
    };
    let here = layout(rack, at, 0.0);
    for (child, (_, child_at)) in rack.devices.iter().zip(&here.device_frames) {
        collect(child, *child_at, found);
    }
    found.push((device.id, here));
}
