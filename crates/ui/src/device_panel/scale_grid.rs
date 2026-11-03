//! Scale's grid (layout L-DEV-051): 13 columns for the notes that come in and
//! 13 rows for the notes that go out, counted from Base, the bottom left cell
//! the root. A column's mapped cell is lit; the black keys' cells are darker.
//! A click on a cell of a User grid sends that column's note to that row, and
//! a click on the lit cell sends it nowhere. A named scale's grid shows its
//! mapping and takes no click.

use bl_devices::remap::{BASE, COLUMNS, SCALE_NAME, map, rows};
use bl_model::ParamId;
use bl_shell::Invocation;
use bl_shell::device_commands::{DeviceFrame, DevicePanel};
use egui::accesskit::Role;
use egui::{Id, Rect, Sense, Ui, pos2, vec2};
use serde_json::json;

use super::{focus_ring, highlight};
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How many columns and rows the grid draws: an octave and the note that
/// begins the next.
const SIDE: u32 = COLUMNS + 1;

/// The cell of column `column` and row `row` in a grid drawn in `rect`; row 0
/// is at the bottom.
fn cell_rect(rect: Rect, column: u32, row: u32) -> Rect {
    let size = vec2(rect.width() / SIDE as f32, rect.height() / SIDE as f32);
    Rect::from_min_size(
        pos2(
            rect.left() + size.x * column as f32,
            rect.bottom() - size.y * (row + 1) as f32,
        ),
        size,
    )
    .shrink(0.5)
}

/// Draws the grid in `rect` and turns a click into `device.set_param`.
pub(super) fn show(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    panel: &DevicePanel,
    (id, rect): (Id, Rect),
    events: &mut Vec<UiEvent>,
) {
    let read = |param: ParamId| panel.read(param);
    let named = read(SCALE_NAME) as usize;
    let lit = rows(named, |c| read(map(c)).round() as i32);
    let base = read(BASE) as u32;
    let user = named == 0 && frame.enabled;
    let response = ui.interact(rect, id, Sense::click());
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    let c = &theme.colors;
    let accent = highlight(theme, frame.enabled);
    let painter = ui.painter_at(rect);
    for column in 0..SIDE {
        for row in 0..SIDE {
            let black = matches!((base + row) % 12, 1 | 3 | 6 | 8 | 10);
            // The last column is the octave of the first, a note higher.
            let mapped = lit[(column % COLUMNS) as usize] + 12 * (column / COLUMNS) as i32;
            let on = mapped == row as i32 && (column < COLUMNS || lit[0] == 0);
            let fill = if on {
                accent
            } else if black {
                c.window.0
            } else {
                c.control.0
            };
            painter.rect_filled(cell_rect(rect, column, row), 0.0, fill);
        }
    }
    if user
        && response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let column =
            (((pointer.x - rect.left()) / rect.width() * SIDE as f32) as u32).min(SIDE - 1);
        let row =
            (((rect.bottom() - pointer.y) / rect.height() * SIDE as f32) as u32).min(SIDE - 1);
        if column < COLUMNS {
            let value = if lit[column as usize] == row as i32 {
                -1
            } else {
                row as i32
            };
            events.push(UiEvent::BeginNamedGesture(
                "Change \"Scale Map\"".to_owned(),
            ));
            events.push(UiEvent::Invoke(Invocation::new(
                "device.set_param",
                json!({ "device": frame.id, "param": map(column), "value": value }),
            )));
            events.push(UiEvent::EndGesture);
        }
    }
    focus_ring(ui, theme, rect, &response);
    let text: Vec<String> = lit
        .iter()
        .enumerate()
        .map(|(c, row)| format!("{c} to {row}"))
        .collect();
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Image);
        node.set_label(bl_devices::remap::GRID_CELL.to_owned());
        node.set_value(text.join(", "));
    });
}
