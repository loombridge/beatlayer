//! The Mapping Browser (WP-7.3; the map modes spec, R8 and R9): in the
//! browser's place while MIDI Map Mode or Key Map Mode is on, a table of the
//! mode's mappings with each one's source, the path to its control, its
//! name, and its Min and Max, which are number boxes.
//!
//! Everything here runs a registered command: a click on a row selects its
//! control (`remote.select`), the number boxes set the range
//! (`remote.set_range`), and the row's menu inverts it
//! (`remote.invert_range`). Delete and Backspace are the keys of the map
//! modes and delete the selected control's mapping.

use bl_model::{RemoteSource, RemoteTarget, fader};
use bl_shell::Shell;
use bl_shell::remote_commands::{MapMode, MappingInfo};
use egui::accesskit::Role;
use egui::{Align2, DragValue, Id, Rect, Sense, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;

const ROW: f32 = 22.0;
const HEAD: f32 = 22.0;
const COLUMNS: f32 = 18.0;
/// The width of a Min or Max box, and the gap after it.
const BOX: f32 = 56.0;
const GAP: f32 = 4.0;

/// The ID of the table.
fn table_id() -> Id {
    Id::new("mapping-browser")
}

/// The ID of a mapping's row.
fn row_id(source: &RemoteSource) -> Id {
    Id::new(("mapping-browser-row", source))
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
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
}

/// Draws the Mapping Browser in `rect`.
pub fn show(ui: &mut Ui, shell: &Shell, theme: &Theme, rect: Rect, events: &mut Vec<UiEvent>) {
    let c = &theme.colors;
    ui.painter()
        .rect_filled(rect, 0.0, c.window.0.gamma_multiply(0.7));
    let mode = shell.map_mode();
    if mode == MapMode::Off {
        return;
    }
    let title = if mode == MapMode::Key {
        "Key Mappings"
    } else {
        "MIDI Mappings"
    };
    ui.painter().text(
        pos2(rect.left() + 8.0, rect.top() + HEAD / 2.0),
        Align2::LEFT_CENTER,
        title,
        theme.small_font(),
        c.text.0,
    );
    // The columns: Source, Path and Name share what the two boxes leave.
    let flexible = (rect.width() - 2.0 * (BOX + GAP) - 8.0).max(60.0);
    let at = [
        rect.left() + 4.0,
        rect.left() + 4.0 + flexible * 0.22,
        rect.left() + 4.0 + flexible * 0.6,
        rect.left() + 4.0 + flexible,
        rect.left() + 4.0 + flexible + BOX + GAP,
        rect.left() + 4.0 + flexible + 2.0 * (BOX + GAP),
    ];
    let columns: [(&str, Rect); 5] = std::array::from_fn(|n| {
        let name = ["Source", "Path", "Name", "Min", "Max"][n];
        let top = rect.top() + HEAD;
        (
            name,
            Rect::from_min_max(pos2(at[n], top), pos2(at[n + 1] - GAP, top + COLUMNS)),
        )
    });
    let table = table_id();
    let body = Rect::from_min_max(pos2(rect.left(), rect.top() + HEAD + COLUMNS), rect.max);
    let _ = ui.interact(rect, table, Sense::hover());
    ui.ctx().accesskit_node_builder(table, |node| {
        node.set_role(Role::Table);
        node.set_label("Mapping Browser".to_owned());
        node.set_bounds(bounds(rect));
    });
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .accessibility_parent(table),
        |ui| {
            for (name, cell) in columns {
                let id = table.with(("column", name));
                let _ = ui.interact(cell, id, Sense::hover());
                ui.painter().text(
                    pos2(cell.left(), cell.center().y),
                    Align2::LEFT_CENTER,
                    name,
                    theme.small_font(),
                    c.text_dim.0,
                );
                ui.ctx().accesskit_node_builder(id, |node| {
                    node.set_role(Role::ColumnHeader);
                    node.set_label(name.to_owned());
                    node.set_bounds(bounds(cell));
                });
            }
            let selected = shell.map_selection();
            let mut y = body.top();
            for info in shell.map_rows() {
                let row =
                    Rect::from_min_size(pos2(rect.left() + 2.0, y), vec2(rect.width() - 4.0, ROW));
                y += ROW;
                if !body.contains_rect(row) {
                    break;
                }
                self::row(ui, theme, &info, row, &at, selected, events);
            }
        },
    );
}

/// One mapping's row.
fn row(
    ui: &mut Ui,
    theme: &Theme,
    info: &MappingInfo,
    row: Rect,
    at: &[f32; 6],
    selected: Option<RemoteTarget>,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let id = row_id(&info.source);
    let response = ui.interact(row, id, Sense::click());
    let here = selected == Some(info.target);
    ui.painter().rect_filled(
        row,
        theme.sizes.corner,
        if here { c.control_hover.0 } else { c.panel.0 },
    );
    if response.clicked() {
        invoke(events, "remote.select", json!({ "target": info.target }));
    }
    let source_text = info.source.label();
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Row);
        node.set_label(format!("{source_text}: {} {}", info.path, info.name));
        node.set_selected(here);
        node.set_bounds(bounds(row));
    });
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(row)
            .accessibility_parent(id),
        |ui| {
            for (n, text) in [source_text.as_str(), &info.path, &info.name]
                .into_iter()
                .enumerate()
            {
                let cell =
                    Rect::from_min_max(pos2(at[n], row.top()), pos2(at[n + 1] - GAP, row.bottom()));
                let cell_id = id.with(("cell", n));
                let _ = ui.interact(cell, cell_id, Sense::hover());
                ui.painter_at(cell).text(
                    pos2(cell.left(), cell.center().y),
                    Align2::LEFT_CENTER,
                    text,
                    theme.small_font(),
                    if n == 0 { c.text.0 } else { c.text_dim.0 },
                );
                ui.ctx().accesskit_node_builder(cell_id, |node| {
                    node.set_role(Role::Cell);
                    node.set_label(text.to_owned());
                    node.set_bounds(bounds(cell));
                });
            }
            for (n, (name, value)) in [("Min", info.min), ("Max", info.max)]
                .into_iter()
                .enumerate()
            {
                let cell =
                    Rect::from_min_size(pos2(at[3 + n], row.top() + 3.0), vec2(BOX, ROW - 6.0));
                number_box(ui, info, name, value, cell, events);
            }
        },
    );
    response.context_menu(|ui| {
        // Spec: ui-parity C-MAP-001.
        if ui.button("Invert Range").clicked() {
            invoke(
                events,
                "remote.invert_range",
                json!({ "source": info.source }),
            );
            ui.close();
        }
    });
}

/// A Min or Max box, in the control's own units: a volume or a send in dB,
/// every other control as its value. A drag is one undo step, and so is a
/// value typed (R9).
fn number_box(
    ui: &mut Ui,
    info: &MappingInfo,
    name: &str,
    value: f64,
    cell: Rect,
    events: &mut Vec<UiEvent>,
) {
    let (low, high) = info.range;
    let whole = matches!(
        info.kind,
        bl_model::ControlKind::Radio { .. } | bl_model::ControlKind::RangedSwitch
    );
    let decibels = matches!(
        info.target,
        RemoteTarget::Send { .. }
            | RemoteTarget::Mixer {
                param: bl_model::MixerParam::Volume,
                ..
            }
            | RemoteTarget::Chain {
                param: bl_model::MixerParam::Volume,
                ..
            }
    );
    let mut shown = value;
    let mut drag = DragValue::new(&mut shown).range(low..=high);
    drag = if whole {
        drag.speed(1.0).max_decimals(0)
    } else if decibels {
        drag.speed(0.005)
            .custom_formatter(|v, _| {
                if v <= 0.0 {
                    "-inf dB".to_owned()
                } else {
                    format!("{:.1} dB", fader::gain_db(v))
                }
            })
            .custom_parser(|text| {
                let text = text.trim().trim_end_matches("dB").trim();
                if text.eq_ignore_ascii_case("-inf") {
                    Some(0.0)
                } else {
                    text.parse::<f64>().ok().map(fader::db_gain)
                }
            })
    } else {
        drag.speed((high - low) / 200.0).max_decimals(2)
    };
    let response = ui.put(cell, drag);
    let id = response.id;
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_label(format!("{name}: {} {}", info.path, info.name));
    });
    if response.drag_started() {
        events.push(UiEvent::BeginNamedGesture(
            "Change Mapping Range".to_owned(),
        ));
    }
    if response.changed() {
        let (min, max) = if name == "Min" {
            (shown, info.max)
        } else {
            (info.min, shown)
        };
        invoke(
            events,
            "remote.set_range",
            json!({ "source": info.source, "min": min, "max": max }),
        );
    }
    if response.drag_stopped() {
        events.push(UiEvent::EndGesture);
    }
}
