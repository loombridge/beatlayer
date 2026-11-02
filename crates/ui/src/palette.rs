//! The colour palette: 5 rows of 14 swatches, drawn the same in the track,
//! clip, scene, chain and macro menus and the Settings window (the themes
//! spec, R15, R22), and under it, in the menus, the row of 14 custom colours
//! and Custom Color…, which opens the colour picker (R28, R31).

use std::hash::Hash;

use bl_model::Colour;
use bl_shell::settings::CustomColours;
use bl_shell::{Invocation, Shell};
use egui::accesskit::Role;
use egui::ecolor::Hsva;
use egui::{
    Color32, Context, Id, Key, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, WidgetInfo,
    WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How many swatches a row has.
pub const COLUMNS: u8 = 14;

/// The width and height of one swatch, in points.
const CELL: f32 = 14.0;

/// The rows of swatches.
const SWATCH_ROWS: u8 = 5;

/// How many swatches the palette has.
const SWATCHES: u8 = SWATCH_ROWS * COLUMNS;

/// The index of the first place of the custom row and of Custom Color…: the
/// cells of the grid after the swatches, in reading order.
const CUSTOM_FIRST: u8 = SWATCHES;
const CUSTOM_COLOR: u8 = SWATCHES + COLUMNS;

/// The space between the swatches and the custom row, and the height of
/// Custom Color….
const GAP: f32 = 6.0;
const BUTTON: f32 = 20.0;

/// What the palette's grid answered.
enum Chosen {
    Swatch(u8),
    Place(usize),
    CustomColor,
}

/// The row and column of cell `index`; Custom Color… is a row of one.
fn row_column(index: u8) -> (u8, u8) {
    if index == CUSTOM_COLOR {
        (SWATCH_ROWS + 1, 0)
    } else {
        (index / COLUMNS, index % COLUMNS)
    }
}

/// The cell at `row` and `column` (clamped to the row).
fn cell_at(row: u8, column: u8) -> u8 {
    if row > SWATCH_ROWS {
        CUSTOM_COLOR
    } else {
        row * COLUMNS + column
    }
}

/// Where cell `index` is, with the palette's top left at `min`.
fn cell_rect(min: egui::Pos2, index: u8) -> Rect {
    if index == CUSTOM_COLOR {
        let top = f32::from(SWATCH_ROWS + 1) * CELL + GAP + GAP / 2.0;
        return Rect::from_min_size(
            min + vec2(0.0, top),
            vec2(f32::from(COLUMNS) * CELL, BUTTON),
        );
    }
    let (row, column) = row_column(index);
    let gap = if row == SWATCH_ROWS { GAP } else { 0.0 };
    Rect::from_min_size(
        min + vec2(f32::from(column) * CELL, f32::from(row) * CELL + gap),
        vec2(CELL, CELL),
    )
}

/// The id of the temporary note of the palette row that has the keyboard and
/// how many rows that palette has, so a menu's Up and Down leave the arrows
/// to the palette while it moves between rows.
fn row_id() -> Id {
    Id::new("palette-row")
}

/// The id of the column the keyboard was in last, for the way back up from
/// Custom Color….
fn column_id() -> Id {
    Id::new("palette-column")
}

/// Whether an open menu leaves Up (`step` below 0) or Down to the palette
/// that has the keyboard: they move between its rows until the focus is on
/// the row they would leave by.
pub(crate) fn keeps_arrow(ctx: &Context, step: i32) -> bool {
    ctx.data(|d| d.get_temp::<(u8, u8)>(row_id()))
        .is_some_and(|(row, rows)| (step > 0 && row + 1 < rows) || (step < 0 && row > 0))
}

/// Draws the palette and returns what was clicked or chosen with Enter. It
/// is one control in the accessibility tree, a grid of buttons named by
/// their row and column; the arrow keys move between them. With `custom`,
/// the grid goes on with the row of custom colours and Custom Color…, as a
/// sixth and seventh row (R31). `salt` tells this palette from others in the
/// same menu.
fn grid(
    ui: &mut Ui,
    theme: &Theme,
    salt: impl Hash + std::fmt::Debug,
    current: Option<Colour>,
    custom: Option<&CustomColours>,
) -> Option<Chosen> {
    let cells = if custom.is_some() {
        CUSTOM_COLOR + 1
    } else {
        SWATCHES
    };
    let rows = if custom.is_some() {
        SWATCH_ROWS + 2
    } else {
        SWATCH_ROWS
    };
    let mut chosen = None;
    ui.scope_builder(UiBuilder::new().id_salt(salt), |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(Role::Grid);
            node.set_label("Colour palette".to_owned());
        });
        let height = if custom.is_some() {
            cell_rect(pos2(0.0, 0.0), CUSTOM_COLOR).bottom()
        } else {
            f32::from(SWATCH_ROWS) * CELL
        };
        let (area, _) =
            ui.allocate_exact_size(vec2(f32::from(COLUMNS) * CELL, height), Sense::hover());
        let base = ui.id();
        let focused = (0..cells).find(|index| ui.memory(|m| m.has_focus(base.with(*index))));
        // Left, Right, Up and Down move between the swatches, and stop at the
        // palette's edges.
        // Only when the keyboard was in the palette before this frame: a menu
        // that moved it in with the same key leaves nothing for the palette.
        let held = ui
            .ctx()
            .data(|d| d.get_temp::<(u8, u8)>(row_id()))
            .is_some();
        if let Some(at) = focused.filter(|_| held) {
            let (row, column) = row_column(at);
            let (dx, dy) = ui.input(|i| {
                (
                    i32::from(i.key_pressed(Key::ArrowRight))
                        - i32::from(i.key_pressed(Key::ArrowLeft)),
                    i32::from(i.key_pressed(Key::ArrowDown))
                        - i32::from(i.key_pressed(Key::ArrowUp)),
                )
            });
            if dx != 0 || dy != 0 {
                let kept = ui
                    .ctx()
                    .data(|d| d.get_temp::<u8>(column_id()))
                    .unwrap_or(column);
                let column = if row == SWATCH_ROWS + 1 { kept } else { column };
                let column = (i32::from(column) + dx).clamp(0, i32::from(COLUMNS) - 1) as u8;
                let row = (i32::from(row) + dy).clamp(0, i32::from(rows) - 1) as u8;
                ui.memory_mut(|m| {
                    // Instead of egui's own move to the nearest widget.
                    m.move_focus(egui::FocusDirection::None);
                    m.request_focus(base.with(cell_at(row, column)));
                });
            }
        }
        ui.ctx().data_mut(|d| match focused {
            Some(at) => {
                let (row, column) = row_column(at);
                d.insert_temp(row_id(), (row, rows));
                if row <= SWATCH_ROWS {
                    d.insert_temp(column_id(), column);
                }
            }
            None => {
                d.remove_temp::<(u8, u8)>(row_id());
            }
        });
        for index in 0..cells {
            let at = cell_rect(area.min, index);
            let response = ui.interact(at, base.with(index), Sense::click());
            // The palette stands in the menu's Up and Down order as the cell
            // that has the keyboard, or its first.
            if focused.map_or(index == 0, |held| held == index) {
                crate::menus::register_item(ui.ctx(), &response);
            }
            let (row, column) = row_column(index);
            let place = usize::from(index.wrapping_sub(CUSTOM_FIRST));
            let own = custom.and_then(|c| {
                (CUSTOM_FIRST..CUSTOM_COLOR)
                    .contains(&index)
                    .then(|| c.get(place))
            });
            let (ticked, name) = if index < SWATCHES {
                (
                    current == Some(Colour::Swatch(index)),
                    format!("Row {}, colour {}", row + 1, column + 1),
                )
            } else if let Some(own) = own {
                let name = match own {
                    Some(rgb) => format!(
                        "Row {}, colour {}, {}",
                        row + 1,
                        column + 1,
                        Colour::hex(rgb)
                    ),
                    None => format!("Row {}, colour {}, empty", row + 1, column + 1),
                };
                (
                    own.is_some_and(|rgb| current == Some(Colour::Custom(rgb))),
                    name,
                )
            } else {
                (false, "Custom Color…".to_owned())
            };
            let painter = ui.painter();
            if index < SWATCHES {
                painter.rect_filled(at.shrink(1.0), 0.0, theme.swatch(index));
            } else if let Some(own) = own {
                match own {
                    Some([r, g, b]) => {
                        painter.rect_filled(at.shrink(1.0), 0.0, Color32::from_rgb(r, g, b));
                    }
                    None => {
                        painter.rect_stroke(
                            at.shrink(1.5),
                            0.0,
                            Stroke::new(1.0, theme.colors.control_hover.0),
                            StrokeKind::Inside,
                        );
                    }
                }
            } else {
                painter.rect_filled(at, theme.sizes.corner, theme.colors.control.0);
                painter.text(
                    at.center(),
                    egui::Align2::CENTER_CENTER,
                    "Custom Color…",
                    egui::FontId::proportional(theme.sizes.small_text),
                    theme.colors.text.0,
                );
            }
            if ticked {
                painter.rect_stroke(
                    at.shrink(0.5),
                    0.0,
                    Stroke::new(1.5, theme.colors.text.0),
                    StrokeKind::Inside,
                );
            }
            response.widget_info(|| {
                if index == CUSTOM_COLOR {
                    WidgetInfo::labeled(WidgetType::Button, true, name.clone())
                } else {
                    WidgetInfo::selected(WidgetType::Button, true, ticked, name.clone())
                }
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
                chosen = if index < SWATCHES {
                    Some(Chosen::Swatch(index))
                } else if index == CUSTOM_COLOR {
                    Some(Chosen::CustomColor)
                } else if own.is_some_and(|o| o.is_some()) {
                    Some(Chosen::Place(place))
                } else {
                    None
                };
            }
        }
    });
    chosen
}

/// Draws the palette of 70 swatches, with swatch `current` ticked, and
/// returns the swatch clicked or chosen with Enter: the one a setting takes,
/// which has no custom colours.
pub fn show_swatches(
    ui: &mut Ui,
    theme: &Theme,
    salt: impl Hash + std::fmt::Debug,
    current: Option<u8>,
) -> Option<u8> {
    match grid(ui, theme, salt, current.map(Colour::Swatch), None) {
        Some(Chosen::Swatch(index)) => Some(index),
        _ => None,
    }
}

/// The command a palette runs and the arguments it takes besides `colour`.
#[derive(Clone, Debug)]
pub struct Target<'a> {
    command: &'a str,
    args: Value,
}

impl<'a> Target<'a> {
    /// A palette that runs `command` with `args`, an object, and the colour.
    #[must_use]
    pub fn new(command: &'a str, args: Value) -> Self {
        Self { command, args }
    }
}

/// Draws the palette with its row of custom colours and Custom Color… under
/// it, `current` ticked, for a menu that colours what `target` names (R16,
/// R28). A swatch, or a filled place of the custom row, runs the command and
/// closes the menu; a custom colour is marked as used in the row. Custom
/// Color… closes the menu and opens the picker, which runs the command when
/// it's confirmed.
pub fn show(
    ui: &mut Ui,
    theme: &Theme,
    shell: &Shell,
    salt: impl Hash + std::fmt::Debug,
    current: Option<Colour>,
    target: Target<'_>,
    events: &mut Vec<UiEvent>,
) {
    let custom = &shell.look().custom_colours;
    let colour = match grid(ui, theme, salt, current, Some(custom)) {
        None => return,
        Some(Chosen::Swatch(index)) => Colour::Swatch(index),
        Some(Chosen::Place(place)) => {
            let Some(rgb) = custom.get(place) else {
                return;
            };
            use_custom(events, rgb);
            Colour::Custom(rgb)
        }
        Some(Chosen::CustomColor) => {
            open_picker(ui.ctx(), theme, &target, current);
            ui.close();
            return;
        }
    };
    events.push(UiEvent::Invoke(Invocation::new(
        target.command,
        with_colour(target.args, colour),
    )));
    ui.close();
}

/// `args` with `colour` in it.
fn with_colour(mut args: Value, colour: Colour) -> Value {
    if let Value::Object(map) = &mut args {
        map.insert("colour".to_owned(), json!(colour));
    }
    args
}

/// Puts a custom colour in the row of the preferences, or marks it as used.
fn use_custom(events: &mut Vec<UiEvent>, rgb: [u8; 3]) {
    events.push(UiEvent::Invoke(Invocation::new(
        bl_shell::commands::SETTINGS_USE_CUSTOM_COLOUR,
        json!({ "colour": Colour::hex(rgb) }),
    )));
}

/// The colour picker's state while it's open: what it will run, and the
/// colour it shows as its red, green and blue, which are what's confirmed,
/// the same as hue, saturation and brightness, and the text of its field.
#[derive(Clone)]
struct Picker {
    command: String,
    args: Value,
    rgb: [u8; 3],
    hsva: Hsva,
    hex: String,
}

fn picker_id() -> Id {
    Id::new("colour-picker")
}

/// Opens the picker on the colour of what's being coloured.
fn open_picker(ctx: &Context, theme: &Theme, target: &Target<'_>, current: Option<Colour>) {
    let colour = theme.colour(current.unwrap_or(Colour::Swatch(0)));
    let rgb = [colour.r(), colour.g(), colour.b()];
    ctx.data_mut(|d| {
        d.insert_temp(
            picker_id(),
            Some(Picker {
                command: target.command.to_owned(),
                args: target.args.clone(),
                rgb,
                hsva: Hsva::from_srgb(rgb),
                hex: Colour::hex(rgb),
            }),
        );
    });
}

fn close_picker(ctx: &Context) {
    ctx.data_mut(|d| {
        d.insert_temp(picker_id(), None::<Picker>);
    });
}

/// Whether the picker is open, which keeps the keys.
#[must_use]
pub fn picker_is_open(ctx: &Context) -> bool {
    ctx.data(|d| {
        d.get_temp::<Option<Picker>>(picker_id())
            .flatten()
            .is_some()
    })
}

/// The picker's hue and saturation square's size, in points.
const SQUARE: egui::Vec2 = vec2(252.0, 140.0);

/// Draws the colour picker over the window if it's open (the themes spec,
/// R28): a hue and saturation square, a brightness slider and a `#RRGGBB`
/// field. OK colours the selection and puts the colour in the row of custom
/// colours; Escape and Cancel leave everything as it was.
pub fn picker(ctx: &Context, theme: &Theme, events: &mut Vec<UiEvent>) {
    let Some(mut state) = ctx
        .data(|d| d.get_temp::<Option<Picker>>(picker_id()))
        .flatten()
    else {
        return;
    };
    let mut done = None;
    egui::Modal::new(picker_id()).show(ctx, |ui| {
        ui.set_width(SQUARE.x);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Custom Color")
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        ui.add_space(8.0);
        let changed = square(ui, theme, &mut state);
        ui.add_space(6.0);
        let mut percent = (state.hsva.v * 100.0).round();
        let slider = ui.add(
            egui::Slider::new(&mut percent, 0.0..=100.0)
                .integer()
                .text("Brightness"),
        );
        if slider.changed() {
            state.hsva.v = percent / 100.0;
            state.rgb = state.hsva.to_srgb();
        }
        if changed || slider.changed() {
            state.hex = Colour::hex(state.rgb);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (swatch, _) = ui.allocate_exact_size(vec2(36.0, 22.0), Sense::hover());
            let [r, g, b] = state.rgb;
            ui.painter()
                .rect_filled(swatch, 0.0, Color32::from_rgb(r, g, b));
            let field = ui.add(
                egui::TextEdit::singleline(&mut state.hex)
                    .id_salt("colour-picker-hex")
                    .hint_text("#RRGGBB")
                    .desired_width(90.0),
            );
            ui.ctx().accesskit_node_builder(field.id, |node| {
                node.set_label("Hex color".to_owned());
            });
            if field.changed()
                && let Some(rgb) = Colour::parse_hex(state.hex.trim())
            {
                state.rgb = rgb;
                state.hsva = Hsva::from_srgb(rgb);
            }
            let valid = Colour::parse_hex(state.hex.trim()).is_some();
            if valid && field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                done = Some(true);
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let valid = Colour::parse_hex(state.hex.trim()).is_some();
            if ui.add_enabled(valid, egui::Button::new("OK")).clicked() {
                done = Some(true);
            }
            if ui.button("Cancel").clicked() {
                done = Some(false);
            }
        });
    });
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        done = Some(false);
    }
    match done {
        Some(true) => {
            events.push(UiEvent::Invoke(Invocation::new(
                &state.command,
                with_colour(state.args.clone(), Colour::Custom(state.rgb)),
            )));
            use_custom(events, state.rgb);
            close_picker(ctx);
        }
        Some(false) => close_picker(ctx),
        None => ctx.data_mut(|d| {
            d.insert_temp(picker_id(), Some(state));
        }),
    }
}

/// The hue and saturation square: hue across, saturation up, at the
/// brightness the slider gives. Click or drag, or the arrow keys, set the
/// colour; returns whether it changed.
fn square(ui: &mut Ui, theme: &Theme, state: &mut Picker) -> bool {
    let (rect, response) = ui.allocate_exact_size(SQUARE, Sense::click_and_drag());
    let id = response.id;
    let mut changed = false;
    if let Some(pointer) = response
        .interact_pointer_pos()
        .filter(|_| response.clicked() || response.dragged())
    {
        state.hsva.h = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        state.hsva.s = (1.0 - (pointer.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        changed = true;
    }
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        let (dx, dy) = ui.input(|i| {
            (
                i32::from(i.key_pressed(Key::ArrowRight))
                    - i32::from(i.key_pressed(Key::ArrowLeft)),
                i32::from(i.key_pressed(Key::ArrowUp)) - i32::from(i.key_pressed(Key::ArrowDown)),
            )
        });
        if dx != 0 || dy != 0 {
            state.hsva.h = (state.hsva.h + dx as f32 / 36.0).clamp(0.0, 1.0);
            state.hsva.s = (state.hsva.s + dy as f32 / 20.0).clamp(0.0, 1.0);
            changed = true;
        }
    }
    if changed {
        state.rgb = state.hsva.to_srgb();
    }
    let (columns, rows) = (36_u32, 8_u32);
    let mut mesh = egui::Mesh::default();
    for j in 0..=rows {
        for i in 0..=columns {
            let h = i as f32 / columns as f32;
            let s = 1.0 - j as f32 / rows as f32;
            let colour = Color32::from(Hsva::new(h, s, state.hsva.v, 1.0));
            mesh.colored_vertex(
                pos2(
                    rect.left() + rect.width() * h,
                    rect.top() + rect.height() * (1.0 - s),
                ),
                colour,
            );
        }
    }
    for j in 0..rows {
        for i in 0..columns {
            let a = j * (columns + 1) + i;
            let below = a + columns + 1;
            mesh.add_triangle(a, a + 1, below);
            mesh.add_triangle(a + 1, below + 1, below);
        }
    }
    let painter = ui.painter();
    painter.add(egui::Shape::mesh(mesh));
    let marker = pos2(
        rect.left() + rect.width() * state.hsva.h,
        rect.top() + rect.height() * (1.0 - state.hsva.s),
    );
    painter.circle_stroke(marker, 5.0, Stroke::new(1.5, Color32::WHITE));
    painter.circle_stroke(marker, 6.5, Stroke::new(1.0, Color32::BLACK));
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.5, theme.colors.focus.0),
            StrokeKind::Outside,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Hue and saturation"));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_value(format!(
            "Hue {}°, saturation {}%",
            (state.hsva.h * 360.0).round(),
            (state.hsva.s * 100.0).round()
        ));
    });
    changed
}
