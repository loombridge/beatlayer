//! Mapping a modulator to its targets, and the Shaper's display (WP-5.10;
//! the modulation spec, R15, R19, R23).
//!
//! A modulator's Map button lights its slot: every parameter that can be a
//! target is tinted in the theme's mapping colour, and a click on one maps
//! it (`modulator.map`). Esc, or the button again, puts the button out.
//! Everything here runs a registered command.

use bl_model::DeviceId;
use bl_model::modulator::{
    ModTarget, ShapePoint, shape_preset, shape_value, with_bend, with_point_added,
    with_point_moved, without_point,
};
use bl_shell::Invocation;
use bl_shell::device_commands::{ArmedMap, DeviceFrame, MappingRow, ModulatorFrame};
use egui::accesskit::Role;
use egui::{
    Align2, Context, CursorIcon, Id, Key, Modifiers, Pos2, Rect, Sense, Shape, Stroke, StrokeKind,
    Ui, pos2, vec2,
};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The lit Map button, as this frame's controls see it.
fn armed_id() -> Id {
    Id::new("lit-map-button")
}

/// Tells the controls drawn this frame which Map button is lit, if one is.
pub fn set_armed(ctx: &Context, armed: Option<ArmedMap>) {
    ctx.data_mut(|d| d.insert_temp(armed_id(), armed));
}

/// The lit Map button, if one is.
#[must_use]
pub fn armed(ctx: &Context) -> Option<ArmedMap> {
    ctx.data(|d| d.get_temp::<Option<ArmedMap>>(armed_id()))
        .flatten()
}

/// Esc puts a lit Map button out and changes nothing (R15), and Enter is
/// kept for the focused target, so its control doesn't act on it too. Call
/// it once a frame, before the controls are drawn. While a value is being
/// typed, or a menu is open (`typing`), both keys stay theirs.
pub fn escape(ctx: &Context, typing: bool, events: &mut Vec<UiEvent>) {
    let typing = typing || ctx.text_edit_focused();
    let entered = armed(ctx).is_some()
        && !typing
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    ctx.data_mut(|d| d.insert_temp(entered_id(), entered));
    let Some(lit) = armed(ctx) else {
        return;
    };
    if !typing && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        events.push(UiEvent::Invoke(disarm(lit)));
    }
}

fn entered_id() -> Id {
    Id::new("modulator-map-enter")
}

fn disarm(lit: ArmedMap) -> Invocation {
    Invocation::new(
        "modulator.arm_map",
        json!({ "device": lit.device, "slot": lit.slot, "armed": false }),
    )
}

/// Tints the control in `rect` while a Map button is lit, and maps it when
/// it's clicked (or Enter is pressed with `control`, the control itself,
/// focused). Returns whether the control is a target now: the caller's own
/// handling of the click is covered by this one. A modulator's own
/// parameters aren't targets (R17).
pub fn overlay(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    control: Id,
    target: ModTarget,
    events: &mut Vec<UiEvent>,
) -> bool {
    let Some(lit) = armed(ui.ctx()) else {
        return false;
    };
    if matches!(target, ModTarget::Device { device, .. } if device == lit.device) {
        return false;
    }
    let colour = theme.colors.mapping.0;
    ui.painter()
        .rect_filled(rect, theme.sizes.corner, colour.gamma_multiply(0.3));
    ui.painter().rect_stroke(
        rect,
        theme.sizes.corner,
        Stroke::new(1.0, colour),
        StrokeKind::Inside,
    );
    let response = ui.interact(rect, control.with("map-target"), Sense::click());
    let entered = ui.memory(|m| m.has_focus(control))
        && ui.data(|d| d.get_temp::<bool>(entered_id()).unwrap_or(false));
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
    }
    if response.clicked() || entered {
        events.push(UiEvent::Invoke(Invocation::new(
            "modulator.map",
            json!({ "device": lit.device, "slot": lit.slot, "target": target }),
        )));
        events.push(UiEvent::Invoke(disarm(lit)));
    }
    true
}

/// The ID of a slot's Map button.
#[must_use]
pub fn map_button_id(device: DeviceId, slot: u8) -> Id {
    Id::new(("modulator-map", device, slot))
}

/// The ID of a slot's Unmap button.
#[must_use]
pub fn unmap_button_id(device: DeviceId, slot: u8) -> Id {
    Id::new(("modulator-unmap", device, slot))
}

/// The ID of a Shaper's shape display.
#[must_use]
pub fn curve_id(device: DeviceId) -> Id {
    Id::new(("modulator-curve", device))
}

/// The ID of the button that draws preset `number`, or Clear for 0.
#[must_use]
pub fn preset_button_id(device: DeviceId, number: u8) -> Id {
    Id::new(("modulator-preset", device, number))
}

/// A small text button.
struct Button<'a> {
    rect: Rect,
    id: Id,
    text: &'a str,
    /// Whether it is lit, in the mapping colour.
    lit: bool,
    enabled: bool,
    /// Its name for assistive technology.
    name: &'a str,
}

impl Button<'_> {
    /// Draws the button; returns whether it was pressed.
    fn show(&self, ui: &mut Ui, theme: &Theme) -> bool {
        let c = &theme.colors;
        let Self {
            rect,
            id,
            text,
            lit,
            enabled,
            name,
        } = *self;
        let response = ui.interact(
            rect,
            id,
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
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
        let ink = match (lit, enabled) {
            (true, _) => c.accent_text.0,
            (false, true) => c.text.0,
            (false, false) => c.text_dim.0,
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
}

/// A modulator's slot, in a cell above its Amount, Min and Max: the target
/// it maps, a Map button that lights, and an Unmap button (R15).
pub fn mapping_cell(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    modulator: &ModulatorFrame,
    slot: u8,
    cell: Rect,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let mapped: Option<&MappingRow> = modulator.mappings.iter().find(|m| m.slot == slot);
    let label = mapped.map_or("Not mapped", |m| m.label.as_str());
    let inner = cell.shrink(4.0);
    ui.painter_at(cell).text(
        pos2(inner.left(), inner.top() + 6.0),
        Align2::LEFT_CENTER,
        format!("{}", slot + 1),
        theme.small_font(),
        c.text_dim.0,
    );
    ui.painter_at(inner).text(
        pos2(inner.left() + 12.0, inner.top() + 6.0),
        Align2::LEFT_CENTER,
        label,
        theme.small_font(),
        if mapped.is_some() {
            c.text.0
        } else {
            c.text_dim.0
        },
    );
    let row = inner.top() + 16.0;
    let map = Rect::from_min_size(pos2(inner.left(), row), vec2(46.0, 16.0));
    let unmap = Rect::from_min_size(pos2(inner.left() + 52.0, row), vec2(54.0, 16.0));
    let lit = modulator.armed == Some(slot);
    let n = slot + 1;
    let pressed = Button {
        rect: map,
        id: map_button_id(frame.id, slot),
        text: "Map",
        lit,
        enabled: frame.enabled,
        name: &format!("Map {n}"),
    }
    .show(ui, theme);
    if pressed {
        events.push(UiEvent::Invoke(Invocation::new(
            "modulator.arm_map",
            json!({ "device": frame.id, "slot": slot }),
        )));
    }
    let pressed = Button {
        rect: unmap,
        id: unmap_button_id(frame.id, slot),
        text: "Unmap",
        lit: false,
        enabled: mapped.is_some(),
        name: &format!("Unmap {n}"),
    }
    .show(ui, theme);
    if pressed {
        events.push(UiEvent::Invoke(Invocation::new(
            "modulator.unmap",
            json!({ "device": frame.id, "slot": slot }),
        )));
    }
}

/// How far a pointer may be from a breakpoint and still hold it, in points.
const GRAB: f32 = 6.0;

/// What a drag in the shape display is doing.
#[derive(Clone, Copy, Debug)]
enum Drag {
    /// Moving breakpoint `index`.
    Move(usize),
    /// Curving the segment from breakpoint `index`, from `bend` at height `y`.
    Bend { index: usize, bend: f64, y: f32 },
}

/// A Shaper's shape display (R23): the breakpoint envelope over one
/// cycle, with a grid. A click adds a breakpoint, a drag moves one,
/// Alt/Option-dragging a segment curves it, Shift-clicking a breakpoint
/// deletes it; Clear and six presets sit along its top. `grid` is the number
/// of divisions of the cycle and `snap` whether breakpoints snap to them.
pub fn curve_cell(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    points: &[ShapePoint],
    cell: Rect,
    (grid, snap): (u32, bool),
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let device = frame.id;
    let ink = if frame.enabled {
        c.accent.0
    } else {
        c.inactive.0
    };
    // Clear and the presets along the top, the grid's choosers below
    // (the cell's own bottom band).
    let top = cell.top() + 3.0;
    let mut x = cell.left() + 3.0;
    let set = |points: Vec<ShapePoint>, events: &mut Vec<UiEvent>| {
        events.push(UiEvent::Invoke(Invocation::new(
            "modulator.set_shape",
            json!({ "device": device, "points": points }),
        )));
    };
    let mut buttons: Vec<(u8, String, f32)> = vec![(0, "Clear".to_owned(), 36.0)];
    buttons.extend((1..=6).map(|n| (n, n.to_string(), 18.0)));
    for (number, text, width) in buttons {
        let rect = Rect::from_min_size(pos2(x, top), vec2(width, 14.0));
        x += width + 3.0;
        let name = if number == 0 {
            "Clear".to_owned()
        } else {
            format!("Shape preset {number}")
        };
        let pressed = Button {
            rect,
            id: preset_button_id(device, number),
            text: &text,
            lit: false,
            enabled: frame.enabled,
            name: &name,
        }
        .show(ui, theme);
        if pressed {
            let points = if number == 0 {
                Vec::new()
            } else {
                shape_preset(number).unwrap_or_default()
            };
            set(points, events);
        }
    }
    let bottom_band = 44.0;
    let area = Rect::from_min_max(
        pos2(cell.left() + 4.0, top + 18.0),
        pos2(cell.right() - 4.0, cell.bottom() - bottom_band),
    );
    if area.width() < 8.0 || area.height() < 8.0 {
        return;
    }
    let painter = ui.painter_at(area.expand(5.0));
    for i in 1..grid {
        let gx = area.left() + area.width() * (i as f32 / grid as f32);
        painter.line_segment(
            [pos2(gx, area.top()), pos2(gx, area.bottom())],
            Stroke::new(1.0, c.separator.0),
        );
    }
    let to_screen = |p: &ShapePoint| {
        pos2(
            area.left() + area.width() * p.x as f32,
            area.bottom() - area.height() * p.y as f32,
        )
    };
    let from_screen = |p: Pos2| {
        (
            f64::from(((p.x - area.left()) / area.width()).clamp(0.0, 1.0)),
            f64::from(((area.bottom() - p.y) / area.height()).clamp(0.0, 1.0)),
        )
    };
    let snapped = |x: f64, y: f64| {
        if snap && grid > 0 {
            ((x * f64::from(grid)).round() / f64::from(grid), y)
        } else {
            (x, y)
        }
    };
    // The curve itself, a point per pixel or so.
    let steps = (area.width() as usize).clamp(16, 256);
    let line: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let x = i as f64 / steps as f64;
            pos2(
                area.left() + area.width() * x as f32,
                area.bottom() - area.height() * shape_value(points, x) as f32,
            )
        })
        .collect();
    painter.add(Shape::line(line, Stroke::new(1.5, ink)));
    for point in points {
        painter.circle(to_screen(point), 3.0, ink, Stroke::new(1.0, c.text.0));
    }

    let id = curve_id(device);
    let response = ui.interact(area, id, Sense::click_and_drag());
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    let drag_id = id.with("drag");
    let near = |at: Pos2| {
        points
            .iter()
            .position(|p| (to_screen(p) - at).length() <= GRAB)
    };
    let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
    if response.drag_started() {
        let origin = ui
            .input(|i| i.pointer.press_origin())
            .unwrap_or_else(|| area.center());
        let drag = match near(origin) {
            Some(index) if !alt => Some(Drag::Move(index)),
            _ if alt && !points.is_empty() => {
                let (x, _) = from_screen(origin);
                let index = points.partition_point(|p| p.x <= x).saturating_sub(1);
                Some(Drag::Bend {
                    index,
                    bend: points[index].bend,
                    y: origin.y,
                })
            }
            _ => None,
        };
        if let Some(drag) = drag {
            ui.data_mut(|d| d.insert_temp(drag_id, drag));
            events.push(UiEvent::BeginNamedGesture("Edit Shape".to_owned()));
        }
    }
    let drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id));
    if let (Some(drag), true, Some(at)) =
        (drag, response.dragged(), response.interact_pointer_pos())
    {
        match drag {
            Drag::Move(index) => {
                let (x, y) = from_screen(at);
                let (x, y) = snapped(x, y);
                set(with_point_moved(points, index, x, y), events);
            }
            Drag::Bend { index, bend, y } => {
                // Dragging up bows the segment up, a full height is the
                // whole range of the bend.
                let moved = f64::from((y - at.y) / area.height()) * 2.0;
                set(with_bend(points, index, bend + moved), events);
            }
        }
    }
    if response.drag_stopped() && drag.is_some() {
        ui.data_mut(|d| d.remove::<Drag>(drag_id));
        events.push(UiEvent::EndGesture);
    }
    if response.clicked()
        && let Some(at) = response.interact_pointer_pos()
    {
        match near(at) {
            Some(index) if shift => {
                events.push(UiEvent::BeginNamedGesture("Edit Shape".to_owned()));
                set(without_point(points, index), events);
                events.push(UiEvent::EndGesture);
            }
            None => {
                let (x, y) = from_screen(at);
                let (x, y) = snapped(x, y);
                events.push(UiEvent::BeginNamedGesture("Edit Shape".to_owned()));
                set(with_point_added(points, x, y), events);
                events.push(UiEvent::EndGesture);
            }
            Some(_) => {}
        }
    }
    let summary = format!(
        "{} breakpoints, {}",
        points.len(),
        points
            .iter()
            .map(|p| format!("{:.0} % at {:.0} %", p.y * 100.0, p.x * 100.0))
            .collect::<Vec<_>>()
            .join(", ")
    );
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Image);
        node.set_label("Curve Shape");
        node.set_value(summary);
    });
}
