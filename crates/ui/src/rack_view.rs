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

use bl_model::{ChainId, DeviceId, RackView, ZoneKind};
use bl_shell::context_menus::{self, CHAIN_COLOUR_ROW};
use bl_shell::device_commands::{
    CHAIN_DROP_AREA_HEIGHT, CHAIN_ROW_HEIGHT, ChainRow, DEVICE_GAP, DeviceFrame,
    RACK_CHAIN_LIST_WIDTH, RACK_COLUMN_WIDTH, RACK_EMPTY_DEVICES_WIDTH, RACK_END_BRACKET_WIDTH,
    RACK_LIST_HEADER_HEIGHT, RACK_MACROS_WIDTH, RACK_ZONE_EDITOR_WIDTH, RackFrame,
    TITLE_BAR_HEIGHT, frame_width, rack_chain_list_width, rack_pads_width,
};
use bl_shell::pad_commands::{DRUM_IO_WIDTH, DRUM_SEND_WIDTH, RETURN_HEADING_HEIGHT};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, Color32, DragAndDrop, Id, Key, Modifiers, Rect, Sense, Shape, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::device_view::DeviceViewUi;
use crate::main_window::UiEvent;
use crate::multisampler::{Handle, bar_parts, drag_zone, note_name, value_at, x_of};
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
    /// A drum chain's Preview button, Receive, Play and Choke choosers, with
    /// Input/Output on (drum racks spec, R21).
    pub io: Option<IoRects>,
    /// A drum chain's send slider to each return chain, with Send on: a
    /// drum rack holds six return chains at most.
    pub sends: [Option<Rect>; bl_model::MAX_RETURN_CHAINS],
    /// A return chain's Audio To chooser.
    pub audio_to: Option<Rect>,
}

/// Where a drum chain's Input/Output controls sit in its row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IoRects {
    /// The Preview button.
    pub preview: Rect,
    /// The Receive chooser.
    pub receive: Rect,
    /// The Play chooser.
    pub play: Rect,
    /// The Choke chooser.
    pub choke: Rect,
}

/// Where a chain list's return section sits (drum racks spec, R7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReturnSectionRects {
    /// The heading above the return chains' rows.
    pub heading: Rect,
    /// The drop area below the return chains.
    pub drop_area: Rect,
}

/// A button in the header above a rack's chain list (racks spec, R35, R37).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneButton {
    /// Shows the key, velocity (Vel) or chain select (Chain) editor.
    Editor(ZoneKind),
    /// Hide: closes the zone editor.
    Hide,
    /// Auto Select.
    AutoSelect,
}

/// Where a rack's chain list and its rows sit.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainListRects {
    /// The whole list.
    pub area: Rect,
    /// The part below the header, which the rows scroll in.
    pub rows_area: Rect,
    /// The header above the rows; empty for a rack whose chains have no
    /// zones.
    pub header: Rect,
    /// The header's buttons.
    pub buttons: Vec<(ZoneButton, Rect)>,
    /// A row per chain shown, those scrolled out of the list included: a
    /// drum rack's return chains' rows only while the Return switch is on.
    pub rows: Vec<ChainRowRects>,
    /// The drop area below the (drum) chains' rows.
    pub drop_area: Rect,
    /// A drum rack's return section, while the Return switch is on.
    pub returns: Option<ReturnSectionRects>,
    /// The height of everything the list holds, for scrolling.
    pub content_height: f32,
}

/// Where a rack's zone editor and its rows sit (racks spec, R35).
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneEditorRects {
    /// The whole editor.
    pub area: Rect,
    /// The zone it shows.
    pub kind: ZoneKind,
    /// The strip above the scale that holds the chain selector, in the
    /// chain select editor.
    pub selector: Rect,
    /// The scale.
    pub scale: Rect,
    /// The span of the scale across the editor, as tall as the editor: what
    /// [`crate::multisampler::x_of`] maps values onto.
    pub bars: Rect,
    /// A row per chain, level with the chain list's.
    pub rows: Vec<(ChainId, Rect)>,
}

/// One shown macro's places in the Macro Controls (macros spec, R20).
#[derive(Clone, Debug, PartialEq)]
pub struct MacroItem {
    /// Its number, from 1.
    pub number: u8,
    /// Its whole slot.
    pub slot: Rect,
    /// The knob's face.
    pub knob: Rect,
    /// Where its name reads.
    pub name: Rect,
    /// Where its value reads.
    pub value: Rect,
    /// Its Map button, shown in Map mode (R3).
    pub map_button: Rect,
}

/// The Macro Controls' controls (macros spec, R20): the Show More, Show
/// Fewer, Map and Rand buttons, and a slot per shown macro.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroLayout {
    /// The whole area.
    pub area: Rect,
    /// Show More.
    pub show_more: Rect,
    /// Show Fewer.
    pub show_fewer: Rect,
    /// The Map switch.
    pub map_switch: Rect,
    /// The Rand button.
    pub rand: Rect,
    /// The shown macros, in number order.
    pub items: Vec<MacroItem>,
}

/// Where each part of a rack sits in its frame.
#[derive(Clone, Debug, PartialEq)]
pub struct RackLayout {
    /// The view selectors.
    pub selectors: Vec<(RackView, Rect)>,
    /// The Macro Controls' place, while it's shown.
    pub macros: Option<Rect>,
    /// The Macro Controls' controls, while it's shown (macros spec, R20).
    pub macro_items: Option<MacroLayout>,
    /// A drum rack's pad view, while it's shown.
    pub pads: Option<crate::drum_view::PadLayout>,
    /// The chain list, while it's shown.
    pub chain_list: Option<ChainListRects>,
    /// The zone editor, while one is shown.
    pub zone_editor: Option<ZoneEditorRects>,
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
        RackView::Pads => "Pad View",
        RackView::InputOutput => "Show/Hide Input/Output",
        RackView::Send => "Show/Hide Sends",
        RackView::Returns => "Show/Hide Returns",
        RackView::AutoSelect => "Auto Select",
    }
}

const VIEWS: [RackView; 3] = [RackView::Macros, RackView::ChainList, RackView::Devices];

/// A drum rack's selectors: the pad view between the Macro Controls and the
/// chain list, and four switches under the views (drum racks spec, R21).
const DRUM_VIEWS: [RackView; 8] = [
    RackView::Macros,
    RackView::Pads,
    RackView::ChainList,
    RackView::Devices,
    RackView::InputOutput,
    RackView::Send,
    RackView::Returns,
    RackView::AutoSelect,
];

/// The selectors a rack's view column has, top to bottom.
fn views_of(rack: &RackFrame) -> &'static [RackView] {
    if rack.pads.is_some() {
        &DRUM_VIEWS
    } else {
        &VIEWS
    }
}

/// Where the `n`th selector of a view column sits: the switches of a drum
/// rack are set a little apart from its views.
fn selector_y(top: f32, n: usize, views: &[RackView]) -> f32 {
    let gap = views
        .iter()
        .position(|v| *v == RackView::InputOutput)
        .filter(|&at| n >= at)
        .map_or(0.0, |_| 5.0);
    top + n as f32 * (BUTTON + 3.0) + gap
}

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

/// The target selected in the mappings list, if one is.
fn listed_target(ctx: &egui::Context) -> Option<(DeviceId, bl_model::MacroTarget)> {
    ctx.data(|d| {
        d.get_temp::<Option<(DeviceId, bl_model::MacroTarget)>>(Id::new("mappings-list-target"))
    })
    .flatten()
}

/// Draws the mappings list in the browser's place while a rack's Map mode is
/// on (macros spec, R3 and R9): one row per mapping with the macro, the
/// device and parameter, and the range, whose number boxes set it; a row's
/// context menu inverts its range or deletes it, as the Delete key does.
pub fn mappings_list(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    ui.painter()
        .rect_filled(rect, 0.0, c.window.0.gamma_multiply(0.7));
    let Some(rack) = shell.devices().map_mode() else {
        return;
    };
    let doc = shell.editor().document();
    let count = shell.devices().views_of(rack).macro_count;
    let Some(state) = shell.devices().macros_of(doc, rack, count) else {
        return;
    };
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), 22.0));
    ui.painter().text(
        pos2(head.left() + 8.0, head.center().y),
        Align2::LEFT_CENTER,
        "Mappings",
        theme.small_font(),
        c.text.0,
    );
    let title = doc
        .device(rack)
        .map_or_else(String::new, |d| d.title().to_owned());
    ui.painter().text(
        pos2(head.right() - 8.0, head.center().y),
        Align2::RIGHT_CENTER,
        title,
        theme.small_font(),
        c.text_dim.0,
    );
    let mut rows = Vec::new();
    for macro_info in &state.macros {
        for mapping in &macro_info.mappings {
            rows.push((macro_info, mapping));
        }
    }
    let selected = listed_target(ui.ctx());
    let row_h = 22.0;
    let mut y = head.bottom();
    for (macro_info, mapping) in rows {
        let row = Rect::from_min_size(pos2(rect.left() + 2.0, y), vec2(rect.width() - 4.0, row_h));
        y += row_h;
        if !rect.contains_rect(row) {
            break;
        }
        let here = selected == Some((rack, mapping.target));
        let id = Id::new(("mapping-row", rack, mapping.target));
        let response = ui.interact(row, id, Sense::click());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        ui.painter().rect_filled(
            row,
            theme.sizes.corner,
            if here { c.control_hover.0 } else { c.panel.0 },
        );
        if response.clicked() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    Id::new("mappings-list-target"),
                    Some((rack, mapping.target)),
                )
            });
        }
        let macro_name = Rect::from_min_size(row.min, vec2(row.width() * 0.22, row.height()));
        let device = Rect::from_min_max(
            pos2(macro_name.right(), row.top()),
            pos2(row.right() - 120.0, row.bottom()),
        );
        ui.painter().text(
            pos2(macro_name.left() + 4.0, row.center().y),
            Align2::LEFT_CENTER,
            &macro_info.name,
            theme.small_font(),
            c.text.0,
        );
        ui.painter().text(
            pos2(device.left() + 4.0, row.center().y),
            Align2::LEFT_CENTER,
            format!("{} · {}", mapping.device, mapping.param),
            theme.small_font(),
            c.text_dim.0,
        );
        // Min and Max, as percentages of the parameter's travel.
        for (at, value) in [
            (row.right() - 116.0, mapping.min_position),
            (row.right() - 58.0, mapping.max_position),
        ] {
            let box_rect =
                Rect::from_min_size(pos2(at, row.top() + 3.0), vec2(52.0, row.height() - 6.0));
            let mut shown = (value * 100.0).round() / 100.0;
            let changed = ui
                .put(
                    box_rect,
                    egui::DragValue::new(&mut shown)
                        .speed(0.01)
                        .range(0.0..=1.0),
                )
                .changed();
            if changed {
                let (min, max) = if at == row.right() - 116.0 {
                    (shown, mapping.max_position)
                } else {
                    (mapping.min_position, shown)
                };
                invoke(
                    events,
                    "rack.set_mapping_range",
                    json!({ "rack": rack, "target": mapping.target, "min": min, "max": max }),
                );
            }
        }
        response.context_menu(|ui| {
            if ui.button("Invert Range").clicked() {
                invoke(
                    events,
                    "rack.invert_mapping",
                    json!({ "rack": rack, "target": mapping.target }),
                );
                ui.close();
            }
            if ui.button("Delete").clicked() {
                invoke(
                    events,
                    "rack.unmap_macro",
                    json!({ "rack": rack, "target": mapping.target }),
                );
                ui.close();
            }
        });
    }
    // A row's Delete key unmaps it (R9).
    let selected = listed_target(ui.ctx());
    if let Some((rack, target)) = selected
        && shell.devices().map_mode() == Some(rack)
        && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete))
    {
        invoke(
            events,
            "rack.unmap_macro",
            json!({ "rack": rack, "target": target }),
        );
    }
}

/// Where a rack's view selectors sit when it's a strip (every view hidden):
/// under the activator.
fn strip_selectors(rect: Rect, views: &[RackView]) -> Vec<(RackView, Rect)> {
    let x = rect.center().x;
    let first = rect.top() + 2.0 + BUTTON + 6.0 + BUTTON / 2.0;
    views
        .iter()
        .enumerate()
        .map(|(n, view)| {
            let y = selector_y(first, n, views);
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
            selectors: strip_selectors(rect, views_of(rack)),
            pads: None,
            macros: None,
            macro_items: None,
            chain_list: None,
            zone_editor: None,
            devices: None,
            device_frames: Vec::new(),
            bracket: None,
        };
    }
    let body = Rect::from_min_max(pos2(rect.left(), rect.top() + TITLE_BAR_HEIGHT), rect.max);
    let column = Rect::from_min_size(body.min, vec2(RACK_COLUMN_WIDTH, body.height()));
    let views = views_of(rack);
    let selectors = views
        .iter()
        .enumerate()
        .map(|(n, view)| {
            let centre = pos2(
                column.center().x,
                selector_y(column.top() + 4.0 + BUTTON / 2.0, n, views),
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
    let pads = (rack_pads_width(rack) > 0.0)
        .then(|| part(rack_pads_width(rack)))
        .and_then(|area| {
            rack.pads
                .as_ref()
                .map(|pads| crate::drum_view::pad_layout(pads, area))
        });
    let chain_list = rack
        .views
        .chain_list
        .then(|| chain_list(rack, part(rack_chain_list_width(rack)), scroll));
    let zone_editor = rack
        .zone_editor()
        .map(|kind| zone_editor(rack, kind, part(RACK_ZONE_EDITOR_WIDTH), scroll));
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
        pads,
        macros,
        macro_items: macros.map(|area| macro_layout(area, rack.views.macro_count)),
        chain_list,
        zone_editor,
        devices,
        device_frames,
        bracket,
    }
}

/// Where the Macro Controls' buttons and each shown macro's slot sit in
/// `area` (macros spec, R20).
fn macro_layout(area: Rect, count: u8) -> MacroLayout {
    let row = Rect::from_min_max(
        pos2(area.left() + 3.0, area.top() + 3.0),
        pos2(area.right() - 3.0, area.top() + 3.0 + 14.0),
    );
    let half = (row.width() - 3.0) / 2.0;
    let show_fewer = Rect::from_min_size(row.min, vec2(half * 0.5 - 1.0, row.height()));
    let show_more = Rect::from_min_size(
        pos2(show_fewer.right() + 2.0, row.top()),
        vec2(half * 0.5 - 1.0, row.height()),
    );
    let rand = Rect::from_min_size(
        pos2(show_more.right() + 2.0, row.top()),
        vec2(half * 0.5 - 1.0, row.height()),
    );
    let map_switch = Rect::from_min_size(
        pos2(rand.right() + 2.0, row.top()),
        vec2(half * 0.5 - 1.0, row.height()),
    );
    let columns = 2;
    let rows = usize::from(count).div_ceil(columns);
    let slot_w = (area.width() - 6.0) / columns as f32;
    let grid_top = row.bottom() + 3.0;
    let slot_h = ((area.bottom() - grid_top - 3.0) / rows.max(1) as f32).min(56.0);
    let mut items = Vec::new();
    for number in 1..=count {
        let index = usize::from(number - 1);
        let (col, r) = (index % columns, index / columns);
        let slot = Rect::from_min_size(
            pos2(
                area.left() + 3.0 + col as f32 * slot_w,
                grid_top + r as f32 * slot_h,
            ),
            vec2(slot_w - 2.0, slot_h - 2.0),
        );
        let name = Rect::from_min_max(slot.min, pos2(slot.right(), slot.top() + 11.0));
        let value = Rect::from_min_max(pos2(slot.left(), slot.bottom() - 11.0), slot.max);
        let knob = Rect::from_min_max(
            pos2(slot.left() + 2.0, name.bottom()),
            pos2(slot.right() - 2.0, value.top()),
        );
        let map_button = Rect::from_min_size(
            pos2(slot.left() + 2.0, value.bottom() - 12.0),
            vec2(slot.width() - 4.0, 12.0),
        );
        items.push(MacroItem {
            number,
            slot,
            knob,
            name,
            value,
            map_button,
        });
    }
    MacroLayout {
        area,
        show_more,
        show_fewer,
        map_switch,
        rand,
        items,
    }
}

/// How tall the header above the chain list is for `rack`.
fn header_height(rack: &RackFrame) -> f32 {
    if rack.has_zones() {
        RACK_LIST_HEADER_HEIGHT
    } else {
        0.0
    }
}

/// The header's buttons in `header`: Key, Vel and Chain as the rack's chains
/// have those zones, and Hide, on the first line; Auto Select on the second.
fn header_buttons(rack: &RackFrame, header: Rect) -> Vec<(ZoneButton, Rect)> {
    if !rack.has_zones() {
        return Vec::new();
    }
    let at = |x: f32, line: f32, width: f32| {
        Rect::from_min_size(
            pos2(header.left() + x, header.top() + line),
            vec2(width, 13.0),
        )
    };
    let first: Vec<ZoneButton> = rack
        .kind
        .zone_kinds()
        .iter()
        .map(|kind| ZoneButton::Editor(*kind))
        .chain([ZoneButton::Hide])
        .collect();
    let mut buttons: Vec<_> = first
        .into_iter()
        .enumerate()
        .map(|(n, button)| (button, at(4.0 + n as f32 * 33.0, 3.0, 30.0)))
        .collect();
    buttons.push((ZoneButton::AutoSelect, at(4.0, 18.0, 66.0)));
    buttons
}

/// What a header button reads.
#[must_use]
pub fn zone_button_name(button: ZoneButton) -> &'static str {
    match button {
        ZoneButton::Editor(ZoneKind::Key) => "Key",
        ZoneButton::Editor(ZoneKind::Velocity) => "Vel",
        ZoneButton::Editor(ZoneKind::Select) => "Chain",
        ZoneButton::Hide => "Hide",
        ZoneButton::AutoSelect => "Auto Select",
    }
}

/// The ID of one of a rack's header buttons.
#[must_use]
pub fn zone_button_id(rack: DeviceId, button: ZoneButton) -> Id {
    Id::new(("rack-zone-button", rack, zone_button_name(button)))
}

/// The ID of a part of a chain's zone bar in the zone editor: `body`, `low`,
/// `high`, `fade-low` or `fade-high`.
#[must_use]
pub fn zone_bar_id(chain: ChainId, part: &str) -> Id {
    Id::new(("rack-zone-bar", chain, part))
}

/// The ID of the chain selector's indicator.
#[must_use]
pub fn selector_indicator_id(rack: DeviceId) -> Id {
    Id::new(("rack-chain-selector", rack))
}

fn zone_editor(rack: &RackFrame, kind: ZoneKind, area: Rect, scroll: f32) -> ZoneEditorRects {
    let header = header_height(rack);
    let selector = Rect::from_min_max(
        pos2(area.left() + 4.0, area.top() + 2.0),
        pos2(area.right() - 4.0, area.top() + 16.0),
    );
    let scale = Rect::from_min_max(
        pos2(area.left() + 4.0, area.top() + 17.0),
        pos2(area.right() - 4.0, area.top() + header.max(18.0) - 1.0),
    );
    let bars = Rect::from_min_max(
        pos2(area.left() + 4.0, area.top()),
        pos2(area.right() - 4.0, area.bottom()),
    );
    let mut y = area.top() + header + 2.0 - scroll;
    let rows = rack
        .chains
        .iter()
        .map(|chain| {
            let row = Rect::from_min_max(
                pos2(area.left() + 2.0, y),
                pos2(area.right() - 2.0, y + CHAIN_ROW_HEIGHT - 2.0),
            );
            y += CHAIN_ROW_HEIGHT;
            (chain.id, row)
        })
        .collect();
    ZoneEditorRects {
        area,
        kind,
        selector,
        scale,
        bars,
        rows,
    }
}

fn chain_list(rack: &RackFrame, area: Rect, scroll: f32) -> ChainListRects {
    let drum = rack.pads.is_some();
    let io = drum && rack.views.input_output;
    let returns = rack.chains.iter().filter(|c| c.output.is_some()).count();
    let sends = if drum && rack.views.send { returns } else { 0 };
    let header = Rect::from_min_size(area.min, vec2(area.width(), header_height(rack)));
    let rows_area = Rect::from_min_max(pos2(area.left(), header.bottom()), area.max);
    let mut y = rows_area.top() + 2.0 - scroll;
    let next = |chain: &ChainRow, y: &mut f32| {
        let row = Rect::from_min_max(
            pos2(area.left() + 2.0, *y),
            pos2(area.right() - 2.0, *y + CHAIN_ROW_HEIGHT - 2.0),
        );
        *y += CHAIN_ROW_HEIGHT;
        row_rects(chain, row, io, sends)
    };
    let mut rows = Vec::new();
    for chain in rack.chains.iter().filter(|c| c.output.is_none()) {
        rows.push(next(chain, &mut y));
    }
    let drop_area = Rect::from_min_max(
        pos2(area.left() + 2.0, y + 2.0),
        pos2(area.right() - 2.0, y + 2.0 + CHAIN_DROP_AREA_HEIGHT),
    );
    y = drop_area.bottom();
    let mut section = None;
    if drum && rack.views.returns {
        let heading = Rect::from_min_max(
            pos2(area.left() + 2.0, y + 2.0),
            pos2(area.right() - 2.0, y + 2.0 + RETURN_HEADING_HEIGHT),
        );
        y = heading.bottom() + 2.0;
        for chain in rack.chains.iter().filter(|c| c.output.is_some()) {
            rows.push(next(chain, &mut y));
        }
        let drop_area = Rect::from_min_max(
            pos2(area.left() + 2.0, y + 2.0),
            pos2(area.right() - 2.0, y + 2.0 + CHAIN_DROP_AREA_HEIGHT),
        );
        y = drop_area.bottom();
        section = Some(ReturnSectionRects { heading, drop_area });
    }
    ChainListRects {
        area,
        rows_area,
        header,
        buttons: header_buttons(rack, header),
        rows,
        drop_area,
        returns: section,
        content_height: y + scroll - rows_area.top() + 4.0,
    }
}

fn row_rects(chain: &ChainRow, row: Rect, io: bool, sends: usize) -> ChainRowRects {
    // The controls every chain has fill the first stretch of the row; a
    // drum rack's columns follow.
    let whole = row;
    let row = if chain.drum.is_some() || chain.output.is_some() {
        Rect::from_min_max(
            row.min,
            pos2(row.left() + RACK_CHAIN_LIST_WIDTH - 4.0, row.bottom()),
        )
    } else {
        row
    };
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
    // The columns right of the row's controls, on its middle line.
    let middle = whole.center().y;
    let cell = |x: f32, width: f32| {
        Rect::from_min_max(pos2(x, middle - 6.0), pos2(x + width, middle + 6.0))
    };
    let mut x = row.right() + 4.0;
    let mut io_rects = None;
    let mut audio_to = None;
    if chain.drum.is_some() && io {
        let preview = Rect::from_center_size(pos2(x + BUTTON / 2.0, middle), vec2(BUTTON, BUTTON));
        let receive = cell(preview.right() + 2.0, 56.0);
        let play = cell(receive.right() + 2.0, 50.0);
        let choke = cell(play.right() + 2.0, 28.0);
        io_rects = Some(IoRects {
            preview,
            receive,
            play,
            choke,
        });
        x += DRUM_IO_WIDTH;
    } else if chain.output.is_some() {
        audio_to = Some(cell(x, DRUM_IO_WIDTH - 6.0));
        x += DRUM_IO_WIDTH;
    } else if io || sends > 0 {
        // A chain without the columns still leaves their room.
        x += if io { DRUM_IO_WIDTH } else { 0.0 };
    }
    let mut send_rects = [None; bl_model::MAX_RETURN_CHAINS];
    if chain.drum.is_some() {
        for (n, slot) in send_rects.iter_mut().enumerate().take(sends) {
            *slot = Some(Rect::from_min_max(
                pos2(x + n as f32 * DRUM_SEND_WIDTH, middle - 5.0),
                pos2(x + (n as f32 + 1.0) * DRUM_SEND_WIDTH - 3.0, middle + 5.0),
            ));
        }
    }
    ChainRowRects {
        chain: chain.id,
        row: whole,
        activator,
        name,
        solo,
        hot_swap,
        volume,
        pan,
        io: io_rects,
        sends: send_rects,
        audio_to,
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

/// A small button of the Macro Controls (macros spec, R20).
#[expect(clippy::too_many_arguments, reason = "the button's inputs")]
fn macro_button(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    id: Id,
    text: &str,
    name: &str,
    lit: bool,
    enabled: bool,
) -> bool {
    let c = &theme.colors;
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    crate::focus::register(ui.ctx(), id, Area::Detail, None);
    let pressed = enabled
        && (response.clicked()
            || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))));
    let face = if lit {
        c.mapping.0
    } else if response.hovered() && enabled {
        c.control_hover.0
    } else {
        c.control.0
    };
    let ink = if lit {
        c.accent_text.0
    } else if enabled {
        c.text.0
    } else {
        c.text_dim.0
    };
    ui.painter().rect_filled(rect, theme.sizes.corner, face);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.small_font(),
        ink,
    );
    if response.has_focus() && crate::focus::outlined(ui.ctx()) {
        ui.painter().rect_stroke(
            rect.expand(1.5),
            theme.sizes.corner,
            Stroke::new(1.0, c.focus.0),
            StrokeKind::Outside,
        );
    }
    let name = name.to_owned();
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Button);
        node.set_label(name);
        node.set_selected(lit);
    });
    pressed
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

/// A drag on a chain's zone bar under way (racks spec, R35).
#[derive(Clone, Copy, Debug)]
pub(crate) struct RackZoneDrag {
    chain: ChainId,
    kind: ZoneKind,
    handle: Handle,
    start_x: f32,
    zone: bl_model::Zone,
    delta: i32,
}

/// A chain's name being typed in its row.
pub(crate) struct ChainRenaming {
    pub chain: ChainId,
    pub text: String,
    pub focused: bool,
}

/// A macro being renamed in its Macro Controls slot (macros spec, R14).
#[derive(Clone, Debug)]
pub(crate) struct MacroRenaming {
    pub rack: DeviceId,
    pub number: u8,
    /// The name being typed; `None` until the slot first shows, when it
    /// starts from the name the macro has.
    pub text: Option<String>,
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
        if let (Some(_), Some(controls)) = (layout.macros, &layout.macro_items) {
            self.macro_controls(ui, shell, theme, frame, rack, controls, events);
        }
        if let (Some(pads), Some(at)) = (&rack.pads, &layout.pads) {
            self.pad_view(ui, shell, theme, frame, pads, at, events);
        }
        if let Some(list) = &layout.chain_list {
            self.chain_list(ui, shell, theme, frame, rack, list, events);
        }
        if let Some(zones) = &layout.zone_editor {
            self.zone_editor(ui, theme, frame, rack, zones, events);
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
            // The pad view: a grid of four.
            RackView::Pads => {
                for (dx, dy) in [(-2.0, -2.0), (2.0, -2.0), (-2.0, 2.0), (2.0, 2.0)] {
                    painter.rect_filled(
                        Rect::from_center_size(c0 + vec2(dx, dy), vec2(3.0, 3.0)),
                        0.0,
                        ink,
                    );
                }
            }
            // Input/Output: arrows in and out.
            RackView::InputOutput => {
                painter.line_segment(
                    [pos2(c0.x - 3.0, c0.y - 1.5), pos2(c0.x + 3.0, c0.y - 1.5)],
                    stroke,
                );
                painter.line_segment(
                    [pos2(c0.x - 3.0, c0.y + 1.5), pos2(c0.x + 3.0, c0.y + 1.5)],
                    stroke,
                );
                painter.circle_filled(pos2(c0.x - 3.0, c0.y - 1.5), 1.0, ink);
                painter.circle_filled(pos2(c0.x + 3.0, c0.y + 1.5), 1.0, ink);
            }
            // Send: a wedge.
            RackView::Send => {
                painter.add(Shape::convex_polygon(
                    vec![
                        c0 + vec2(-3.0, -3.0),
                        c0 + vec2(3.0, 0.0),
                        c0 + vec2(-3.0, 3.0),
                    ],
                    ink,
                    Stroke::NONE,
                ));
            }
            // Return: a curve back.
            RackView::Returns => {
                painter.line_segment(
                    [pos2(c0.x + 3.0, c0.y - 2.0), pos2(c0.x - 3.0, c0.y - 2.0)],
                    stroke,
                );
                painter.line_segment(
                    [pos2(c0.x - 3.0, c0.y - 2.0), pos2(c0.x - 3.0, c0.y + 2.0)],
                    stroke,
                );
                painter.line_segment(
                    [pos2(c0.x - 3.0, c0.y + 2.0), pos2(c0.x + 3.0, c0.y + 2.0)],
                    stroke,
                );
            }
            // Auto Select: a pointer on a row.
            RackView::AutoSelect => {
                painter.rect_stroke(
                    Rect::from_center_size(c0, vec2(7.0, 4.0)),
                    0.5,
                    stroke,
                    StrokeKind::Inside,
                );
                painter.circle_filled(c0 + vec2(2.0, 2.0), 1.5, ink);
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
        let content = list.content_height;
        if area_response.hovered() || ui.rect_contains_pointer(list.area) {
            let delta = ui.input(|i| i.smooth_scroll_delta.y);
            if delta != 0.0 {
                let max = (content - list.rows_area.height()).max(0.0);
                let now = (self.chain_scroll(frame.id) - delta).clamp(0.0, max);
                self.chain_scrolls.insert(frame.id, now);
            }
        }
        self.chain_keys(ui, shell, rack, events);
        let drag = DragAndDrop::payload::<ChainDrag>(ui.ctx());
        for rects in &list.rows {
            let Some(row) = rack.chains.iter().find(|c| c.id == rects.chain) else {
                continue;
            };
            if rects.row.bottom() < list.rows_area.top()
                || rects.row.top() > list.rows_area.bottom()
            {
                continue;
            }
            self.chain_row(ui, shell, theme, rack, row, rects, list, events);
        }
        self.return_section(ui, theme, frame, rack, list, events);
        self.chain_drop(ui, theme, frame, rack, list, drag.as_deref(), events);
        self.zone_header(ui, theme, frame, rack, list, events);
    }

    /// The buttons above the chain list: Key, Vel, Chain and Hide, and Auto
    /// Select (racks spec, R35, R37).
    fn zone_header(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        rack: &RackFrame,
        list: &ChainListRects,
        events: &mut Vec<UiEvent>,
    ) {
        if list.buttons.is_empty() {
            return;
        }
        let c = &theme.colors;
        let painter = ui.painter_at(list.header);
        painter.rect_filled(list.header, 0.0, c.panel.0);
        for (button, at) in &list.buttons {
            let id = zone_button_id(frame.id, *button);
            let response = ui.interact(*at, id, Sense::click());
            crate::focus::register(ui.ctx(), id, Area::Detail, None);
            let on = match button {
                ZoneButton::Editor(kind) => rack.zone_editor() == Some(*kind),
                ZoneButton::Hide => rack.zone_editor().is_none(),
                ZoneButton::AutoSelect => rack.views.auto_select,
            };
            if response.clicked()
                || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
            {
                match button {
                    ZoneButton::Editor(kind) => invoke(
                        events,
                        "rack.set_zone_editor",
                        json!({ "rack": frame.id, "editor": kind }),
                    ),
                    ZoneButton::Hide => {
                        invoke(events, "rack.set_zone_editor", json!({ "rack": frame.id }));
                    }
                    ZoneButton::AutoSelect => invoke(
                        events,
                        "rack.set_auto_select",
                        json!({ "rack": frame.id, "on": !on }),
                    ),
                }
            }
            let face = if on {
                c.accent.0
            } else if response.hovered() {
                c.control_hover.0
            } else {
                c.window.0
            };
            painter.rect_filled(*at, theme.sizes.corner, face);
            let name = zone_button_name(*button);
            painter.text(
                at.center(),
                Align2::CENTER_CENTER,
                name,
                theme.small_font(),
                if on { c.accent_text.0 } else { c.text_dim.0 },
            );
            if response.has_focus() && crate::focus::outlined(ui.ctx()) {
                painter.rect_stroke(
                    at.expand(1.5),
                    theme.sizes.corner,
                    Stroke::new(1.0, c.focus.0),
                    StrokeKind::Outside,
                );
            }
            response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, name));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Button);
                node.set_label(name.to_owned());
                node.set_description(
                    match button {
                        ZoneButton::Editor(_) => "Shows this zone editor beside the chain list",
                        ZoneButton::Hide => "Closes the zone editor",
                        ZoneButton::AutoSelect => {
                            "Selects the chains that are processing in the chain list"
                        }
                    }
                    .to_owned(),
                );
            });
        }
    }

    /// The zone editor right of the chain list (racks spec, R35): the scale
    /// over a row per chain, each with its zone bar, and in the chain select
    /// editor the draggable chain selector above the scale.
    fn zone_editor(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        rack: &RackFrame,
        zones: &ZoneEditorRects,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let kind = zones.kind;
        let painter = ui.painter_at(zones.area);
        painter.rect_filled(zones.area, 0.0, c.window.0.gamma_multiply(0.5));
        let group = Id::new(("rack-zone-editor", frame.id));
        ui.interact(zones.area, group, Sense::hover());
        ui.ctx().accesskit_node_builder(group, |node| {
            node.set_role(Role::Group);
            node.set_label(format!(
                "{} Zone Editor of {}",
                match kind {
                    ZoneKind::Key => "Key",
                    ZoneKind::Velocity => "Velocity",
                    ZoneKind::Select => "Chain Select",
                },
                frame.title
            ));
        });
        // The scale.
        let ruler_id = Id::new(("rack-zone-scale", frame.id));
        let ruler = ui.interact(zones.scale, ruler_id, Sense::click());
        let (first, last) = kind.range();
        for value in (first..=last).step_by(1) {
            let tick = match kind {
                ZoneKind::Key => value % 12 == 0,
                ZoneKind::Velocity | ZoneKind::Select => value % 32 == 0 || value == last,
            };
            if !tick {
                continue;
            }
            let x = x_of(zones.bars, kind, f32::from(value));
            painter.line_segment(
                [
                    pos2(x, zones.scale.bottom() - 4.0),
                    pos2(x, zones.scale.bottom()),
                ],
                Stroke::new(1.0, c.text_dim.0),
            );
            let label = match kind {
                ZoneKind::Key if value % 36 == 0 => Some(note_name(value)),
                ZoneKind::Key => None,
                _ => Some(value.to_string()),
            };
            if let Some(label) = label {
                painter.text(
                    pos2(x + 1.0, zones.scale.top()),
                    Align2::LEFT_TOP,
                    label,
                    theme.small_font(),
                    c.text_dim.0,
                );
            }
        }
        if kind == ZoneKind::Select {
            // Chain Selector Filters MIDI Ctrl is in the chain select
            // ruler's context menu (racks spec, R31).
            ruler.context_menu(|ui| {
                let mut on = rack.filters_midi_ctrl;
                if ui
                    .checkbox(&mut on, "Chain Selector Filters MIDI Ctrl")
                    .changed()
                {
                    invoke(
                        events,
                        "device.set_param",
                        json!({
                            "device": frame.id,
                            "param": bl_model::FILTER_MIDI_CTRL,
                            "value": f64::from(u8::from(on)),
                        }),
                    );
                    ui.close();
                }
            });
            self.chain_selector(ui, theme, frame, rack, zones, events);
        }
        // A row per chain, level with the chain list's.
        for (chain, (id, row)) in rack.chains.iter().zip(&zones.rows) {
            debug_assert_eq!(chain.id, *id);
            if row.bottom() < zones.area.top() + RACK_LIST_HEADER_HEIGHT
                || row.top() > zones.area.bottom()
            {
                continue;
            }
            self.zone_bar(ui, theme, zones, chain, *row, events);
        }
        self.update_zone_drag(ui, zones, events);
    }

    /// The chain selector's indicator above the scale: dragging it sets the
    /// rack's chain selector (racks spec, R33, R35), the drag one undo step.
    fn chain_selector(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        rack: &RackFrame,
        zones: &ZoneEditorRects,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let kind = zones.kind;
        let x = x_of(zones.bars, kind, f32::from(rack.selector) + 0.5);
        let id = selector_indicator_id(frame.id);
        let at = Rect::from_center_size(
            pos2(x, zones.selector.center().y),
            vec2(10.0, zones.selector.height()),
        );
        let response = ui.interact(at.union(zones.selector), id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let painter = ui.painter_at(zones.area);
        painter.add(Shape::convex_polygon(
            vec![
                pos2(x - 5.0, zones.selector.top() + 1.0),
                pos2(x + 5.0, zones.selector.top() + 1.0),
                pos2(x, zones.selector.bottom()),
            ],
            c.focus.0,
            Stroke::NONE,
        ));
        painter.line_segment(
            [
                pos2(x, zones.selector.bottom()),
                pos2(x, zones.area.bottom()),
            ],
            Stroke::new(1.0, c.focus.0.gamma_multiply(0.6)),
        );
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Slider);
            node.set_label("Chain Selector".to_owned());
            node.set_value(rack.selector.to_string());
        });
        if response.has_focus() {
            // The keyboard's form of the drag: Left and Right step the
            // selector, each press its own undo step.
            let step = ui.input_mut(|i| {
                i32::from(i.consume_key(Modifiers::NONE, Key::ArrowRight))
                    - i32::from(i.consume_key(Modifiers::NONE, Key::ArrowLeft))
            });
            let value = (i32::from(rack.selector) + step).clamp(0, 127);
            if step != 0 && value != i32::from(rack.selector) {
                invoke(
                    events,
                    "device.set_param",
                    json!({
                        "device": frame.id,
                        "param": bl_model::CHAIN_SELECTOR,
                        "value": value,
                    }),
                );
            }
        }
        if response.drag_started() {
            events.push(UiEvent::BeginNamedGesture(
                "Change \"Chain Selector\"".to_owned(),
            ));
        }
        if (response.dragged() || response.clicked())
            && let Some(pointer) = ui.input(|i| i.pointer.latest_pos())
        {
            let value = value_at(zones.bars, kind, pointer.x);
            if value != i32::from(rack.selector) {
                invoke(
                    events,
                    "device.set_param",
                    json!({
                        "device": frame.id,
                        "param": bl_model::CHAIN_SELECTOR,
                        "value": value,
                    }),
                );
            }
        }
        if response.drag_stopped() {
            events.push(UiEvent::EndGesture);
        }
    }

    /// One chain's zone bar: its shape, and the handles for its edges, its
    /// body and its fades (racks spec, R35).
    fn zone_bar(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        zones: &ZoneEditorRects,
        chain: &ChainRow,
        row: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let kind = zones.kind;
        let zone = chain.zones.get(kind);
        let parts = bar_parts(zones.bars, row, kind, zone);
        let painter = ui.painter_at(Rect::from_min_max(
            pos2(
                zones.area.left(),
                zones.area.top() + RACK_LIST_HEADER_HEIGHT,
            ),
            zones.area.max,
        ));
        painter.rect_filled(row, theme.sizes.corner, c.control.0.gamma_multiply(0.6));
        let (left, right) = (parts.body.left(), parts.body.right());
        let fade_left = x_of(
            zones.bars,
            kind,
            f32::from(zone.low) + f32::from(zone.fade_low),
        );
        let fade_right = x_of(
            zones.bars,
            kind,
            f32::from(zone.high) + 1.0 - f32::from(zone.fade_high),
        );
        let (top, bottom) = (row.top() + 2.0, row.bottom() - 1.0);
        let colour = chain.colour.map_or(c.control_hover.0, |n| swatch(theme, n));
        painter.add(Shape::convex_polygon(
            vec![
                pos2(left, bottom),
                pos2(fade_left.max(left), top),
                pos2(fade_right.min(right).max(fade_left), top),
                pos2(right, bottom),
            ],
            colour,
            Stroke::new(1.0, c.separator.0),
        ));
        let name = format!(
            "{} {}",
            chain.name,
            crate::multisampler::zone_text(kind, zone)
        );
        // Later widgets sit above earlier ones where they overlap: the body
        // first, then the edges over it, then the fade handles.
        for (part, rect, handle) in [
            ("body", parts.body, Handle::Body),
            ("low", parts.low, Handle::Low),
            ("high", parts.high, Handle::High),
            ("fade-low", parts.fade_low, Handle::FadeLow),
            ("fade-high", parts.fade_high, Handle::FadeHigh),
        ] {
            let id = zone_bar_id(chain.id, part);
            let response = ui.interact(rect.intersect(zones.area), id, Sense::click_and_drag());
            if matches!(handle, Handle::Low | Handle::High) {
                let _ = response
                    .clone()
                    .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            }
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Slider);
                node.set_label(format!("{name}, {part}"));
                node.set_description(
                    "Left and Right move this edge, the zone or this fade by one value".to_owned(),
                );
            });
            // The keyboard's form of the drag: Left and Right move the part
            // by a value, each press its own undo step.
            crate::focus::register(ui.ctx(), id, Area::Detail, None);
            if response.has_focus() {
                let step = ui.input_mut(|i| {
                    i32::from(i.consume_key(Modifiers::NONE, Key::ArrowRight))
                        - i32::from(i.consume_key(Modifiers::NONE, Key::ArrowLeft))
                });
                if step != 0 {
                    invoke(
                        events,
                        "chain.set_zone",
                        json!({
                            "chain": chain.id,
                            "kind": kind,
                            "zone": drag_zone(zone, handle, step, kind),
                        }),
                    );
                }
                if crate::focus::outlined(ui.ctx()) {
                    painter.rect_stroke(
                        rect.expand(1.0),
                        0.0,
                        Stroke::new(1.0, c.focus.0),
                        StrokeKind::Outside,
                    );
                }
            }
            if response.drag_started()
                && let Some(pointer) = ui.input(|i| i.pointer.press_origin())
            {
                self.zone_drag = Some(RackZoneDrag {
                    chain: chain.id,
                    kind,
                    handle,
                    start_x: pointer.x,
                    zone,
                    delta: 0,
                });
                events.push(UiEvent::BeginGesture("Change Chain Zone"));
            }
        }
    }

    /// Carries a drag on a zone on: the zone follows the pointer as the
    /// handle moves it, and the release ends the one undo step.
    fn update_zone_drag(&mut self, ui: &Ui, zones: &ZoneEditorRects, events: &mut Vec<UiEvent>) {
        let Some(drag) = self.zone_drag.as_mut() else {
            return;
        };
        let (pointer, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
        if let Some(pointer) = pointer {
            let (low, high) = drag.kind.range();
            let per_value = zones.bars.width() / (f32::from(high - low) + 1.0);
            let delta = ((pointer.x - drag.start_x) / per_value).round() as i32;
            if delta != drag.delta {
                drag.delta = delta;
                invoke(
                    events,
                    "chain.set_zone",
                    json!({
                        "chain": drag.chain,
                        "kind": drag.kind,
                        "zone": drag_zone(drag.zone, drag.handle, delta, drag.kind),
                    }),
                );
            }
        }
        if !down {
            self.zone_drag = None;
            events.push(UiEvent::EndGesture);
        }
    }

    /// Up and Down step the selection while the chain list has the focus
    /// (racks spec, R22).
    /// The Macro Controls (macros spec, R2, R3 and R20): Show More, Show
    /// Fewer, Rand and the Map switch, and a knob per shown macro with its
    /// name above and its value below; in Map mode, a Map button under each.
    #[expect(clippy::too_many_arguments, reason = "the controls' inputs")]
    fn macro_controls(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        rack: &RackFrame,
        layout: &MacroLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let painter = ui.painter_at(layout.area);
        painter.rect_filled(layout.area.shrink(1.0), 0.0, c.window.0.gamma_multiply(0.5));
        let group = Id::new(("rack-macros", frame.id));
        ui.interact(layout.area, group, Sense::hover());
        ui.ctx().accesskit_node_builder(group, |node| {
            node.set_role(Role::Group);
            node.set_label("Macro Controls".to_owned());
        });
        let map_mode = shell.devices().map_mode() == Some(frame.id);
        let selected = shell
            .devices()
            .macro_selected()
            .filter(|(rack, _)| *rack == frame.id)
            .map(|(_, number)| number);
        let count = rack.views.macro_count;
        let press =
            |ui: &mut Ui, rect: Rect, id: Id, text: &str, name: &str, lit: bool, enabled: bool| {
                macro_button(ui, theme, rect, id, text, name, lit, enabled)
            };
        if press(
            ui,
            layout.show_fewer,
            Id::new(("macros-fewer", frame.id)),
            "-",
            "Show Fewer Macros",
            false,
            count > 1,
        ) {
            invoke(
                events,
                "rack.set_macro_count",
                json!({ "rack": frame.id, "count": count - 1 }),
            );
        }
        if press(
            ui,
            layout.show_more,
            Id::new(("macros-more", frame.id)),
            "+",
            "Show More Macros",
            false,
            count < bl_model::MACROS,
        ) {
            invoke(
                events,
                "rack.set_macro_count",
                json!({ "rack": frame.id, "count": count + 1 }),
            );
        }
        if press(
            ui,
            layout.rand,
            Id::new(("macros-rand", frame.id)),
            "Rand",
            "Randomize Macros",
            false,
            true,
        ) {
            invoke(events, "rack.randomize_macros", json!({ "rack": frame.id }));
        }
        if press(
            ui,
            layout.map_switch,
            Id::new(("macros-map-switch", frame.id)),
            "Map",
            "Map Mode",
            map_mode,
            true,
        ) {
            invoke(
                events,
                "rack.map_mode",
                json!({ "rack": frame.id, "on": !map_mode }),
            );
        }
        for item in &layout.items {
            self.macro_slot(
                ui, shell, theme, frame, rack, item, map_mode, selected, events,
            );
        }
    }

    /// One macro's slot: its name, knob and value, and its Map button in Map
    /// mode.
    #[expect(clippy::too_many_arguments, reason = "the slot's inputs")]
    fn macro_slot(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        rack: &RackFrame,
        item: &MacroItem,
        map_mode: bool,
        selected: Option<u8>,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let info = rack
            .macros
            .as_ref()
            .and_then(|state| state.macros.iter().find(|m| m.number == item.number));
        let name = info.map_or_else(|| format!("Macro {}", item.number), |m| m.name.clone());
        let value = info.map_or(0.0, |m| m.value);
        let text = info.map_or_else(|| "0".to_owned(), |m| m.text.clone());
        let painter = ui.painter_at(item.slot);
        let here = selected == Some(item.number);
        painter.rect_filled(
            item.slot,
            theme.sizes.corner,
            if here { c.control_hover.0 } else { c.panel.0 },
        );
        // Rename types the name in place; Return runs the command, Esc
        // cancels (R14).
        let renaming = self
            .renaming_macro
            .as_mut()
            .filter(|r| r.rack == frame.id && r.number == item.number);
        if let Some(renaming) = renaming {
            let mut text = renaming.text.take().unwrap_or_else(|| name.clone());
            let edit = ui.put(
                item.name,
                egui::TextEdit::singleline(&mut text)
                    .id(Id::new(("macro-name", frame.id, item.number)))
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
                        "rack.rename_macro",
                        json!({
                            "rack": frame.id,
                            "macro_number": item.number,
                            "name": text,
                        }),
                    );
                }
                self.renaming_macro = None;
            } else {
                if let Some(renaming) = self.renaming_macro.as_mut() {
                    renaming.text = Some(text);
                }
            }
            return;
        }
        let ink = if frame.enabled {
            c.text.0
        } else {
            c.text_dim.0
        };
        painter.text(
            item.name.center(),
            Align2::CENTER_CENTER,
            &name,
            theme.small_font(),
            ink,
        );
        let id = Id::new(("macro-knob", frame.id, item.number));
        let response = ui.interact(item.knob, id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let param = bl_model::macro_param(item.number);
        if response.drag_started() {
            events.push(UiEvent::BeginNamedGesture(format!("Change \"{name}\"")));
        }
        if response.dragged() {
            let delta = f64::from(response.drag_delta().y) / 100.0;
            let moved = (value - delta).clamp(0.0, 1.0);
            invoke(
                events,
                "device.set_param",
                json!({ "device": frame.id, "param": param, "value": moved }),
            );
        }
        if response.drag_stopped() {
            events.push(UiEvent::EndGesture);
        }
        if response.double_clicked() {
            invoke(
                events,
                "device.set_param",
                json!({ "device": frame.id, "param": param, "value": 0.0 }),
            );
        }
        if response.clicked() {
            invoke(
                events,
                "rack.select_macro",
                json!({ "rack": frame.id, "macro_number": item.number }),
            );
        }
        let colour = info
            .and_then(|m| m.colour)
            .map_or(c.accent.0, |index| swatch(theme, index));
        let centre = item.knob.center();
        let radius = (item.knob.height().min(item.knob.width()) / 2.0 - 2.0).max(4.0);
        let (from, sweep) = (
            std::f32::consts::PI * 0.5 + std::f32::consts::PI / 6.0,
            std::f32::consts::PI * 5.0 / 3.0,
        );
        let arc = |to: f32| -> Vec<egui::Pos2> {
            let steps = 24;
            (0..=steps)
                .map(|i| {
                    let a = from + sweep * to * i as f32 / steps as f32;
                    centre + vec2(a.cos(), a.sin()) * radius
                })
                .collect()
        };
        painter.add(Shape::line(arc(1.0), Stroke::new(2.0, c.control.0)));
        if value > 0.0 {
            painter.add(Shape::line(arc(value as f32), Stroke::new(2.0, colour)));
        }
        let a = from + sweep * value as f32;
        painter.line_segment(
            [centre, centre + vec2(a.cos(), a.sin()) * radius],
            Stroke::new(1.5, c.text.0),
        );
        painter.text(
            item.value.center(),
            Align2::CENTER_CENTER,
            &text,
            theme.small_font(),
            ink,
        );
        if response.has_focus() && crate::focus::outlined(ui.ctx()) {
            ui.painter().rect_stroke(
                item.knob.expand(1.5),
                theme.sizes.corner,
                Stroke::new(1.0, c.focus.0),
                StrokeKind::Outside,
            );
        }
        response.widget_info(|| WidgetInfo::slider(true, value, &name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Slider);
            node.set_label(name.clone());
            node.set_value(text.clone());
            node.set_numeric_value(value);
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(1.0);
            node.add_action(egui::accesskit::Action::Increment);
            node.add_action(egui::accesskit::Action::Decrement);
        });
        response.context_menu(|ui| {
            if ui.button("Rename").clicked() {
                self.renaming_macro = Some(MacroRenaming {
                    rack: frame.id,
                    number: item.number,
                    text: None,
                    focused: false,
                });
                ui.close();
            }
            let excluded = info.is_some_and(|m| m.randomize_excluded);
            let label = if excluded {
                "Include in Randomization"
            } else {
                "Exclude Macro from Randomization"
            };
            if ui.button(label).clicked() {
                invoke(
                    events,
                    "rack.set_macro_randomize_excluded",
                    json!({
                        "rack": frame.id,
                        "macro_number": item.number,
                        "excluded": !excluded,
                    }),
                );
                ui.close();
            }
            ui.menu_button("Colour", |ui| {
                for index in 0..PALETTE_COLUMNS as u8 {
                    let at = Rect::from_min_size(ui.cursor().min, vec2(14.0, 14.0));
                    let response = ui.allocate_rect(at, Sense::click());
                    ui.painter()
                        .rect_filled(at.shrink(1.0), 0.0, swatch(theme, index));
                    if response.clicked() {
                        invoke(
                            events,
                            "rack.set_macro_colour",
                            json!({
                                "rack": frame.id,
                                "macro_number": item.number,
                                "colour": index,
                            }),
                        );
                        ui.close();
                    }
                }
            });
            if ui.button("None").clicked() {
                invoke(
                    events,
                    "rack.set_macro_colour",
                    json!({
                        "rack": frame.id,
                        "macro_number": item.number,
                        "colour": null,
                    }),
                );
                ui.close();
            }
            if !info.is_some_and(|m| m.mappings.is_empty()) {
                ui.separator();
            }
        });
        if map_mode {
            let pressed = macro_button(
                ui,
                theme,
                item.map_button,
                Id::new(("macro-map", frame.id, item.number)),
                "Map",
                &format!("Map Macro {}", item.number),
                false,
                true,
            );
            if pressed && let Some((device, param)) = crate::mapping::macro_target(ui.ctx()) {
                invoke(
                    events,
                    "rack.map_macro",
                    json!({
                        "rack": frame.id,
                        "macro_number": item.number,
                        "target": { "type": "device", "device": device, "param": param },
                    }),
                );
                crate::mapping::set_macro_target(ui.ctx(), None);
            }
        }
        let _ = shell;
    }

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
        let clip = list.rows_area;
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
        self.drum_cells(ui, theme, rack, row, rects, events);
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
    pub(crate) fn slider_keys(ui: &Ui, response: &egui::Response) -> (i32, bool) {
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
                // move below lands among chains that stay put: in a drum
                // rack, the end of its drum chains, before its returns.
                let last = rack
                    .chains
                    .iter()
                    .filter(|c| c.output.is_none())
                    .count()
                    .saturating_sub(1);
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
