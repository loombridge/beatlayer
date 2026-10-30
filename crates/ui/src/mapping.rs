//! Mapping a modulator to its targets, and the Shaper's display (WP-5.10;
//! the modulation spec, R15, R19, R23).
//!
//! A modulator's Map button lights its slot: every parameter that can be a
//! target is tinted in the theme's mapping colour, and a click on one maps
//! it (`modulator.map`). Esc, or the button again, puts the button out.
//! Everything here runs a registered command.

use bl_model::modulator::{
    ModTarget, STRIKE_MOD, ShapePoint, shape_preset, shape_value, with_bend, with_point_added,
    with_point_moved, with_sustain, without_point,
};
use bl_model::{ClipControl, ControlKind, DeviceId, DeviceKind, ParamId, RemoteTarget};
use bl_shell::Invocation;
use bl_shell::device_commands::{ArmedMap, DeviceFrame, MappingRow, ModulatorFrame};
use bl_shell::remote_commands::MapMode;
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
    let entered = (armed(ctx).is_some() || remote_map(ctx).is_some())
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

/// A rack's Map mode, as this frame's controls see it (macros spec, R3): the
/// rack, and the devices inside it whose parameters can be mapped.
#[derive(Clone, Debug)]
pub struct MacroMap {
    /// The rack whose Map mode is on.
    pub rack: DeviceId,
    /// The devices inside it, at any depth, whose parameters are mappable.
    pub devices: std::collections::BTreeSet<DeviceId>,
}

fn macro_map_id() -> Id {
    Id::new("macro-map-mode")
}

fn macro_target_id() -> Id {
    Id::new("macro-map-target")
}

/// Tells the controls drawn this frame which rack's Map mode is on, if one
/// is.
pub fn set_macro_map(ctx: &Context, map: Option<MacroMap>) {
    ctx.data_mut(|d| d.insert_temp(macro_map_id(), map));
}

/// The rack whose Map mode is on, if one is.
#[must_use]
pub fn macro_map(ctx: &Context) -> Option<MacroMap> {
    ctx.data(|d| d.get_temp::<Option<MacroMap>>(macro_map_id()))
        .flatten()
}

/// The parameter selected in Map mode, waiting for a macro's Map button
/// (macros spec, R5).
#[must_use]
pub fn macro_target(ctx: &Context) -> Option<(DeviceId, ParamId)> {
    ctx.data(|d| d.get_temp::<Option<(DeviceId, ParamId)>>(macro_target_id()))
        .flatten()
}

/// The macros that hold parameters, by device and parameter (macros spec,
/// R11).
fn held_id() -> Id {
    Id::new("macro-held-params")
}

/// Tells the controls drawn this frame which parameters a macro holds.
pub fn set_held(ctx: &Context, held: std::collections::BTreeMap<(DeviceId, ParamId), String>) {
    ctx.data_mut(|d| d.insert_temp(held_id(), held));
}

/// The macro that holds `device`'s `param`, by name, if one does.
#[must_use]
pub fn held(ctx: &Context, device: DeviceId, param: ParamId) -> Option<String> {
    ctx.data(|d| d.get_temp::<std::collections::BTreeMap<(DeviceId, ParamId), String>>(held_id()))
        .and_then(|held| held.get(&(device, param)).cloned())
}

fn hover_id() -> Id {
    Id::new("macro-hover-hold")
}

/// Notes the macro named under the pointer, for the Status Bar.
pub fn set_hover_hold(ctx: &Context, name: Option<String>) {
    ctx.data_mut(|d| d.insert_temp(hover_id(), name));
}

/// The macro named under the pointer, if one is (macros spec, R11).
#[must_use]
pub fn hover_hold(ctx: &Context) -> Option<String> {
    ctx.data(|d| d.get_temp::<Option<String>>(hover_id()))
        .flatten()
}

fn hover_lock_id(shown: bool) -> Id {
    Id::new(if shown {
        "surface-lock-hover-shown"
    } else {
        "surface-lock-hover-next"
    })
}

/// Notes what the locked device's hand icon under the pointer says, for the
/// Status Bar to read in the next frame: the Status Bar draws before the
/// detail area that notes it.
pub fn set_hover_lock(ctx: &Context, text: Option<String>) {
    ctx.data_mut(|d| d.insert_temp(hover_lock_id(false), text));
}

/// Starts a frame: what the last frame noted is what the Status Bar shows.
pub fn rotate_hover_lock(ctx: &Context) {
    ctx.data_mut(|d| {
        let next = d.get_temp::<Option<String>>(hover_lock_id(false)).flatten();
        d.insert_temp(hover_lock_id(true), next);
        d.insert_temp(hover_lock_id(false), None::<String>);
    });
}

/// What the hand icon the pointer was over in the last frame says, if there
/// was one (device chains R42).
#[must_use]
pub fn hover_lock(ctx: &Context) -> Option<String> {
    ctx.data(|d| d.get_temp::<Option<String>>(hover_lock_id(true)))
        .flatten()
}

/// Selects the parameter Map mode's next Map button maps, or none.
pub fn set_macro_target(ctx: &Context, target: Option<(DeviceId, ParamId)>) {
    ctx.data_mut(|d| d.insert_temp(macro_target_id(), target));
}

/// Tints a parameter that a rack's Map mode can map, and selects it when
/// it's clicked (macros spec, R3 and R5). Returns whether the control is
/// mappable now: the caller's own handling of the click still runs, so a
/// click changes nothing.
pub fn macro_overlay(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    device: DeviceId,
    param: ParamId,
) -> bool {
    let Some(map) = macro_map(ui.ctx()) else {
        return false;
    };
    if !map.devices.contains(&device) {
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
    let selected = macro_target(ui.ctx()) == Some((device, param));
    if selected {
        ui.painter().rect_stroke(
            rect.expand(1.0),
            theme.sizes.corner,
            Stroke::new(2.0, theme.colors.accent.0),
            StrokeKind::Outside,
        );
    }
    let response = ui.interact(
        rect,
        Id::new(("macro-target", device, param.0)),
        Sense::click(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
    }
    if response.clicked() {
        set_macro_target(ui.ctx(), Some((device, param)));
    }
    true
}

/// A map mode (MIDI Map Mode or Key Map Mode), as this frame's controls see
/// it (map modes spec, R3).
#[derive(Clone, Debug)]
pub struct RemoteMap {
    /// The mode that is on.
    pub mode: MapMode,
    /// The control selected for learning, if one is.
    pub selected: Option<RemoteTarget>,
    /// The source each mapped control's overlay reads.
    pub labels: std::collections::HashMap<RemoteTarget, String>,
}

fn remote_map_id() -> Id {
    Id::new("remote-map-mode")
}

/// Tells the controls drawn this frame which map mode is on, if one is.
pub fn set_remote_map(ctx: &Context, map: Option<RemoteMap>) {
    ctx.data_mut(|d| d.insert_temp(remote_map_id(), map.map(std::sync::Arc::new)));
}

/// The map mode that is on, if one is.
#[must_use]
pub fn remote_map(ctx: &Context) -> Option<std::sync::Arc<RemoteMap>> {
    ctx.data(|d| d.get_temp::<Option<std::sync::Arc<RemoteMap>>>(remote_map_id()))
        .flatten()
}

/// Tints `control`'s `rect` while a map mode is on and selects it when it is
/// clicked, or when Enter is pressed with `control` focused, instead of
/// operating it (map modes spec, R3, R4). `continuous` says it is a
/// continuous control, which Key Map Mode leaves as it is, except that a
/// click does nothing. Returns whether the control is mappable now.
pub fn remote_overlay(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    control: Id,
    target: RemoteTarget,
    continuous: bool,
    events: &mut Vec<UiEvent>,
) -> bool {
    let Some(map) = remote_map(ui.ctx()) else {
        return false;
    };
    let id = control.with("remote-target");
    // The overlay takes the click and the drag the control would have.
    let response = ui.interact(rect, id, Sense::click_and_drag());
    if continuous && map.mode == MapMode::Key {
        return false;
    }
    let (colour, name) = match map.mode {
        MapMode::Key => (theme.colors.key_map.0, "Key Map Mode"),
        _ => (theme.colors.midi_map.0, "MIDI Map Mode"),
    };
    ui.painter()
        .rect_filled(rect, theme.sizes.corner, colour.gamma_multiply(0.3));
    ui.painter().rect_stroke(
        rect,
        theme.sizes.corner,
        Stroke::new(1.0, colour),
        StrokeKind::Inside,
    );
    let label = map.labels.get(&target);
    if let Some(label) = label {
        ui.painter_at(rect).text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            theme.small_font(),
            theme.colors.text.0,
        );
    }
    if map.selected == Some(target) {
        ui.painter().rect_stroke(
            rect.expand(1.0),
            theme.sizes.corner,
            Stroke::new(2.0, theme.colors.accent.0),
            StrokeKind::Outside,
        );
    }
    let description = format!(
        "{name}: {}",
        label.map_or_else(|| "not mapped".to_owned(), |l| format!("mapped to {l}"))
    );
    ui.ctx().accesskit_node_builder(control, |node| {
        node.set_description(description);
    });
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
    }
    let entered = ui.memory(|m| m.has_focus(control))
        && ui.data(|d| d.get_temp::<bool>(entered_id()).unwrap_or(false));
    if response.clicked() || entered {
        events.push(UiEvent::Invoke(Invocation::new(
            "remote.select",
            json!({ "target": target }),
        )));
    }
    true
}

/// The map modes' overlay on one of the Clip View's controls, `control`
/// (map modes R3, R11, R17): a mapping acts on the clips the Clip View shows.
pub fn clip_overlay(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    id: Id,
    control: ClipControl,
    events: &mut Vec<UiEvent>,
) -> bool {
    remote_overlay(
        ui,
        theme,
        rect,
        id,
        RemoteTarget::Clip { control },
        control.kind() == ControlKind::Continuous,
        events,
    )
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
        // The sustain breakpoint is drawn larger (R26).
        let radius = if point.sustain { 5.0 } else { 3.0 };
        painter.circle(to_screen(point), radius, ink, Stroke::new(1.0, c.text.0));
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
    let (alt, shift, command) =
        ui.input(|i| (i.modifiers.alt, i.modifiers.shift, i.modifiers.command));
    // Spec: modulation R26. Only a Shaper MIDI has a sustain breakpoint.
    let sustains =
        matches!(&frame.kind, DeviceKind::Native { device_type } if device_type == STRIKE_MOD);
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
            // Cmd/Ctrl-click makes a breakpoint the sustain breakpoint, or
            // takes the mark away (R26).
            Some(index) if command && sustains => {
                events.push(UiEvent::BeginNamedGesture("Edit Shape".to_owned()));
                set(with_sustain(points, index), events);
                events.push(UiEvent::EndGesture);
            }
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
