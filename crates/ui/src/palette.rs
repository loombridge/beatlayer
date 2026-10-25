//! The colour palette: 5 rows of 14 swatches, drawn the same in the track,
//! clip, chain and macro menus and the Settings window (the themes spec,
//! R15, R22).

use std::hash::Hash;

use egui::accesskit::Role;
use egui::{Sense, Stroke, StrokeKind, Ui, UiBuilder, WidgetInfo, WidgetType, vec2};

use crate::theme::Theme;

/// How many swatches a row has.
pub const COLUMNS: u8 = 14;

/// The width and height of one swatch, in points.
const CELL: f32 = 14.0;

/// The id of the temporary note of the palette row that has the keyboard, so
/// a menu's Up and Down leave the arrows to the palette while it moves
/// between rows.
fn row_id() -> egui::Id {
    egui::Id::new("palette-row")
}

/// Whether an open menu leaves Up (`step` below 0) or Down to the palette
/// that has the keyboard: they move between its rows until the focus is on
/// the row they would leave by.
pub(crate) fn keeps_arrow(ctx: &egui::Context, step: i32) -> bool {
    ctx.data(|d| d.get_temp::<u8>(row_id()))
        .is_some_and(|row| (step > 0 && row < 4) || (step < 0 && row > 0))
}

/// Draws the palette with swatch `current` ticked, and returns the swatch
/// clicked or chosen with Enter. It is one control in the accessibility
/// tree, a grid of buttons named by their row and column; the arrow keys
/// move between them. `salt` tells this palette from others in the same menu.
pub fn show(
    ui: &mut Ui,
    theme: &Theme,
    salt: impl Hash + std::fmt::Debug,
    current: Option<u8>,
) -> Option<u8> {
    let mut chosen = None;
    ui.scope_builder(UiBuilder::new().id_salt(salt), |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(Role::Grid);
            node.set_label("Colour palette".to_owned());
        });
        let (area, _) =
            ui.allocate_exact_size(vec2(f32::from(COLUMNS) * CELL, 5.0 * CELL), Sense::hover());
        let base = ui.id();
        let focused =
            (0..bl_model::SWATCHES).find(|index| ui.memory(|m| m.has_focus(base.with(*index))));
        // Left, Right, Up and Down move between the swatches, and stop at the
        // palette's edges.
        // Only when the keyboard was in the palette before this frame: a menu
        // that moved it in with the same key leaves nothing for the palette.
        let held = ui.ctx().data(|d| d.get_temp::<u8>(row_id())).is_some();
        if let Some(at) = focused.filter(|_| held) {
            let (column, row) = (at % COLUMNS, at / COLUMNS);
            let (dx, dy) = ui.input(|i| {
                (
                    i32::from(i.key_pressed(egui::Key::ArrowRight))
                        - i32::from(i.key_pressed(egui::Key::ArrowLeft)),
                    i32::from(i.key_pressed(egui::Key::ArrowDown))
                        - i32::from(i.key_pressed(egui::Key::ArrowUp)),
                )
            });
            if dx != 0 || dy != 0 {
                let column = (i32::from(column) + dx).clamp(0, i32::from(COLUMNS) - 1) as u8;
                let row = (i32::from(row) + dy).clamp(0, 4) as u8;
                ui.memory_mut(|m| {
                    // Instead of egui's own move to the nearest widget.
                    m.move_focus(egui::FocusDirection::None);
                    m.request_focus(base.with(row * COLUMNS + column));
                });
            }
        }
        ui.ctx().data_mut(|d| match focused {
            Some(at) => {
                d.insert_temp(row_id(), at / COLUMNS);
            }
            None => {
                d.remove_temp::<u8>(row_id());
            }
        });
        for index in 0..bl_model::SWATCHES {
            let (column, row) = (index % COLUMNS, index / COLUMNS);
            let at = egui::Rect::from_min_size(
                area.min + vec2(f32::from(column) * CELL, f32::from(row) * CELL),
                vec2(CELL, CELL),
            );
            let response = ui.interact(at, base.with(index), Sense::click());
            // The palette stands in the menu's Up and Down order as the swatch
            // that has the keyboard, or its first.
            if focused.map_or(index == 0, |held| held == index) {
                crate::menus::register_item(ui.ctx(), &response);
            }
            ui.painter()
                .rect_filled(at.shrink(1.0), 0.0, theme.swatch(index));
            let ticked = current == Some(index);
            if ticked {
                ui.painter().rect_stroke(
                    at.shrink(0.5),
                    0.0,
                    Stroke::new(1.5, theme.colors.text.0),
                    StrokeKind::Inside,
                );
            }
            response.widget_info(|| {
                WidgetInfo::selected(
                    WidgetType::Button,
                    true,
                    ticked,
                    format!("Row {}, colour {}", row + 1, column + 1),
                )
            });
            if response.has_focus() {
                ui.painter().rect_stroke(
                    at,
                    0.0,
                    Stroke::new(1.0, theme.colors.accent.0),
                    StrokeKind::Inside,
                );
            }
            if response.clicked() {
                chosen = Some(index);
            }
        }
    });
    chosen
}
