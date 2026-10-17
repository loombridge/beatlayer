//! The generic device UI (WP-2.5; the device framework spec, R15 to R20):
//! any described device drawn in its frame from its parameters'
//! descriptors and its layout of control bands and display cells (IMP-14).
//!
//! What Live 12.4.6 shows was observed on 12 October 2026: a knob's name
//! above it and its value below; a drag is relative, 0.8% of the range per
//! point (about 125 points end to end, as the mixer's pan); a double-click,
//! or Delete with the knob selected, sets the default; each change is an
//! undo step named `Change "<name>"`; switches are rectangular text buttons,
//! lit in the accent while on; choosers are dark boxes with the value and a
//! ▼; number boxes are dark boxes with the value centred and a fill showing
//! where it sits in its range; a display cell is black with a thin border.
//! Switched off, a device's knob arcs, lit buttons, number boxes' fill, and
//! display curves turn grey while its names and values stay as they were.
//! What isn't observed yet (each in the spec's "To observe"): the knob's
//! size and arc, Shift's factor (3, as pan's), the Status Bar's text while
//! dragging (the mixer's `<Name>: <value>`), and the spacing.
//!
//! Every change runs `device.set_param`, which an agent runs too.

use std::f32::consts::PI;

use bl_model::device_layout::{Size, Widget};
use bl_model::modulator::ModTarget;
use bl_model::{DeviceDisplay, DeviceId, ParamDescriptor, ParamId, ParamKind};
use bl_shell::Invocation;
use bl_shell::device_commands::{DeviceFrame, DevicePanel};
use egui::accesskit::{Action, Role};
use egui::{
    Align2, Color32, Event, Id, Key, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{modulated_text, paint_ring_segment, ring_segment};

/// Points of vertical drag that move a control from one end of its range to
/// the other (observed in Live 12.4.6 on Dry/Wet: 0.8% of the range per
/// point, as the mixer's pan).
pub const DRAG_TRAVEL: f32 = 125.0;

/// How much finer a drag is with Shift held: 3, as on the mixer's pan.
/// (Not yet observed on a device's knob.)
pub const FINE: f32 = 3.0;

/// The undo step a change to `param` makes (observed: `Change "Dry/Wet"`).
#[must_use]
pub fn gesture_name(param: &ParamDescriptor) -> String {
    format!("Change \"{}\"", param.name)
}

/// What the Status Bar shows while `param` is dragged to `value`, in the
/// mixer's form (`Track Volume: -21.0 dB`).
#[must_use]
pub fn readout(param: &ParamDescriptor, value: f64) -> String {
    format!("{}: {}", param.name, param.format(value))
}

/// The colour of a control's lit highlight (a knob's arc, a switch that's
/// on): the accent while the device is on, and grey while it's switched off
/// (observed).
#[must_use]
pub fn highlight(theme: &Theme, enabled: bool) -> Color32 {
    if enabled {
        theme.colors.accent.0
    } else {
        theme.colors.inactive.0
    }
}

/// The ID of a parameter's control in a device's panel.
#[must_use]
pub fn param_control_id(device: DeviceId, param: ParamId) -> Id {
    Id::new(("device-param", device, param))
}

/// The ID of a display cell in a device's panel.
#[must_use]
pub fn cell_id(device: DeviceId, index: usize) -> Id {
    Id::new(("device-cell", device, index))
}

/// Where a device's body sits in its frame: below the title bar.
#[must_use]
pub fn body_rect(frame: Rect, title_bar: f32) -> Rect {
    Rect::from_min_max(pos2(frame.left(), frame.top() + title_bar), frame.max)
}

/// Each control's parameter, widget, and place in a panel.
pub type ControlRects = Vec<(ParamId, Widget, Rect)>;

/// Each display cell's name and place in a panel.
pub type CellRects = Vec<(String, Rect)>;

/// Where each control and display cell sits in a body at `body`: the
/// layout placed at the body's size ([`DeviceLayout::place`]), so docked it
/// sits at its docked size, and a floating window (WP-2.5b) passes a larger
/// body.
///
/// [`DeviceLayout::place`]: bl_model::DeviceLayout::place
#[must_use]
pub fn place(panel: &DevicePanel, body: Rect) -> (ControlRects, CellRects) {
    place_shown(panel, false, body)
}

/// The same for a device whose expanded part shows while `expanded`
/// (WP-5.42).
#[must_use]
pub fn place_shown(panel: &DevicePanel, expanded: bool, body: Rect) -> (ControlRects, CellRects) {
    let placed = panel
        .layout(expanded)
        .place(Size::new(body.width(), body.height()));
    let at = |r: bl_model::device_layout::Rect| {
        Rect::from_min_size(body.min + vec2(r.x, r.y), vec2(r.width, r.height))
    };
    (
        placed
            .controls
            .iter()
            .map(|c| (c.param, c.widget, at(c.rect)))
            .collect(),
        placed
            .cells
            .iter()
            .map(|c| (c.name.clone(), at(c.rect)))
            .collect(),
    )
}

/// A drag under way: where the value started on its travel, and how far
/// the pointer has moved, in travel.
#[derive(Clone, Copy, Debug, Default)]
struct Drag {
    start: f64,
    moved: f64,
}

/// Draws a device's panel in `body`, and turns what the person does into
/// `device.set_param` invocations. `display` is what the device's display
/// shows now, if its type draws one. `played` gives the base and the played
/// value of a parameter while a clip modulates it.
pub fn show(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    display: Option<&DeviceDisplay>,
    body: Rect,
    played: &dyn Fn(ParamId) -> Option<(f64, f64)>,
    events: &mut Vec<UiEvent>,
) {
    let Some(panel) = frame.panel.as_ref() else {
        return;
    };
    let c = &theme.colors;
    let (controls, cells) = place_shown(panel, frame.expanded, body);
    for (index, (name, rect)) in cells.into_iter().enumerate() {
        // A display cell: black, with a thin border (observed). What it
        // shows is the device's own to draw.
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, c.display.0);
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, c.separator.0),
            StrokeKind::Inside,
        );
        let id = cell_id(frame.id, index);
        let _ = ui.interact(rect, id, Sense::hover());
        // Spec: modulation R15, R23. A modulator's cells show its slots'
        // mappings and, for the Curve Mod, its shape.
        if let Some(modulator) = &frame.modulator {
            let slot = name
                .strip_prefix("Mapping ")
                .and_then(|n| n.parse::<u8>().ok());
            if let Some(slot) = slot {
                crate::mapping::mapping_cell(ui, theme, frame, modulator, slot - 1, rect, events);
            } else if let Some(points) = modulator.shape.as_ref().filter(|_| name == "Curve") {
                let value = |param: ParamId| {
                    panel
                        .description
                        .params
                        .iter()
                        .position(|p| p.id == param)
                        .map_or(0.0, |i| panel.value(i))
                };
                let grid = bl_model::modulator::GRIDS
                    .get(value(bl_model::modulator::GRID_DIVISIONS) as usize)
                    .map_or(4, |&(_, n)| n);
                let snap = value(bl_model::modulator::SNAP) >= 0.5;
                crate::mapping::curve_cell(ui, theme, frame, points, rect, (grid, snap), events);
            }
        }
        // Spec: device-framework R36. The device's own drawing fills its
        // first cell, and a screen reader hears its readings.
        let shown = display.filter(|d| index == 0 && has_drawing(d));
        if let Some(display) = shown {
            crate::device_display::draw(
                ui,
                theme,
                rect,
                id,
                frame.enabled,
                display,
                &panel.description.views,
                events,
            );
        }
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Image);
            node.set_label(name);
            if let Some(display) = shown {
                node.set_value(display.text());
            }
        });
    }
    for (param, widget, rect) in controls {
        let Some(index) = panel.description.params.iter().position(|p| p.id == param) else {
            continue;
        };
        let descriptor = &panel.description.params[index];
        let value = panel.value(index);
        let control = Control {
            device: frame.id,
            enabled: frame.enabled,
            descriptor,
            value,
            modulated: played(param),
            rect,
        };
        match widget {
            Widget::Knob => control.knob(ui, theme, events),
            Widget::Number => control.number(ui, theme, events),
            Widget::Toggle => control.toggle(ui, theme, events),
            Widget::Chooser => control.chooser(ui, theme, events),
        }
        // Spec: modulation R15. While a Map button is lit, every parameter
        // but the modulator's own is a target.
        crate::mapping::overlay(
            ui,
            theme,
            rect,
            param_control_id(frame.id, param),
            ModTarget::Device {
                device: frame.id,
                param,
            },
            events,
        );
    }
}

/// Whether a display has anything to draw.
fn has_drawing(display: &DeviceDisplay) -> bool {
    !(display.series.is_empty() && display.readings.is_empty())
}

/// One control being drawn.
struct Control<'a> {
    device: DeviceId,
    enabled: bool,
    descriptor: &'a ParamDescriptor,
    value: f64,
    /// Its base and what it plays while a clip modulates it.
    modulated: Option<(f64, f64)>,
    rect: Rect,
}

impl Control<'_> {
    fn id(&self) -> Id {
        param_control_id(self.device, self.descriptor.id)
    }

    fn set(&self, value: f64) -> Invocation {
        Invocation::new(
            "device.set_param",
            json!({ "device": self.device, "param": self.descriptor.id, "value": value }),
        )
    }

    /// Sets the value as one undo step named for the parameter.
    fn change(&self, value: f64, events: &mut Vec<UiEvent>) {
        let value = self.descriptor.clamp(value);
        if value == self.value {
            return;
        }
        events.push(UiEvent::BeginNamedGesture(gesture_name(self.descriptor)));
        events.push(UiEvent::Invoke(self.set(value)));
        events.push(UiEvent::EndGesture);
    }

    /// The name above the control; it stays as it is when the device is
    /// switched off (observed).
    fn label(&self, ui: &Ui, theme: &Theme) -> Rect {
        let top = Rect::from_min_size(self.rect.min, vec2(self.rect.width(), 12.0));
        ui.painter().text(
            top.center(),
            Align2::CENTER_CENTER,
            &self.descriptor.name,
            theme.small_font(),
            theme.colors.text.0,
        );
        Rect::from_min_max(pos2(self.rect.left(), top.bottom()), self.rect.max)
    }

    /// The colour of a lit highlight: the accent, or grey while the device
    /// is switched off (observed).
    fn lit(&self, theme: &Theme) -> Color32 {
        highlight(theme, self.enabled)
    }

    /// A drag, a double-click, Delete, the arrow keys, and a screen
    /// reader's steps on a value control (knobs and number boxes).
    fn value_input(&self, ui: &Ui, response: &Response, events: &mut Vec<UiEvent>) {
        let d = self.descriptor;
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        // While a value control has the keyboard, Delete sets its default
        // rather than deleting the device (S-DEV-006).
        if response.gained_focus() {
            events.push(UiEvent::FocusValue(true));
        }
        if response.lost_focus() {
            events.push(UiEvent::FocusValue(false));
        }
        let drag_id = response.id.with("drag");
        if response.drag_started() {
            // The pointer moves a little before egui calls it a drag; that
            // counts too, so the drag is the pointer's whole movement.
            let before = ui.input(|i| {
                let origin = i.pointer.press_origin()?;
                let now = i.pointer.interact_pos()?;
                Some(now.y - origin.y)
            });
            let early = before.unwrap_or(0.0) - response.drag_delta().y;
            ui.data_mut(|data| {
                data.insert_temp(
                    drag_id,
                    Drag {
                        start: d.normalize(self.value),
                        moved: f64::from(-early / DRAG_TRAVEL),
                    },
                );
            });
            events.push(UiEvent::BeginNamedGesture(gesture_name(d)));
        }
        if response.dragged() {
            let dy = response.drag_delta().y;
            let fine = ui.input(|i| i.modifiers.shift);
            let drag = ui.data_mut(|data| {
                let drag = data.get_temp_mut_or_default::<Drag>(drag_id);
                drag.moved += f64::from(-dy / DRAG_TRAVEL / if fine { FINE } else { 1.0 });
                *drag
            });
            let value = d.denormalize(drag.start + drag.moved);
            crate::mixer::show_readout(ui.ctx(), readout(d, value));
            if dy != 0.0 && value != self.value {
                events.push(UiEvent::Invoke(self.set(value)));
            }
        }
        if response.drag_stopped() {
            ui.data_mut(|data| data.remove::<Drag>(drag_id));
            events.push(UiEvent::EndGesture);
        }
        let id = response.id;
        let mut steps = ui.input(|i| {
            i.num_accesskit_action_requests(id, Action::Increment) as i32
                - i.num_accesskit_action_requests(id, Action::Decrement) as i32
        });
        let mut reset = response.double_clicked();
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        ..Default::default()
                    },
                );
            });
            ui.input(|i| {
                for event in &i.events {
                    if let Event::Key {
                        key, pressed: true, ..
                    } = event
                    {
                        match key {
                            Key::ArrowUp => steps += 1,
                            Key::ArrowDown => steps -= 1,
                            Key::Delete | Key::Backspace => reset = true,
                            _ => {}
                        }
                    }
                }
            });
        }
        if reset {
            self.change(d.default, events);
        } else if steps != 0 {
            self.change(step(d, self.value, steps), events);
        }
    }

    /// The value the control shows: its base while it's modulated.
    fn shown(&self) -> f64 {
        self.modulated.map_or(self.value, |(base, _)| base)
    }

    fn describe(&self, ui: &Ui, response: &Response, role: Role) {
        let d = self.descriptor;
        let text = modulated_text(
            d.format(self.shown()),
            self.modulated.map(|(_, played)| d.format(played)),
        );
        response.widget_info(|| WidgetInfo::slider(true, self.value, &d.name));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(role);
            node.set_label(d.name.clone());
            node.set_value(text);
            node.set_numeric_value(self.shown());
            node.set_min_numeric_value(d.min());
            node.set_max_numeric_value(d.max());
            node.add_action(Action::Increment);
            node.add_action(Action::Decrement);
            if !d.description.is_empty() {
                node.set_description(d.description.clone());
            }
        });
    }

    /// A knob: its name above, an arc showing the value, the value below.
    fn knob(&self, ui: &mut Ui, theme: &Theme, events: &mut Vec<UiEvent>) {
        let below = self.label(ui, theme);
        let value_row = Rect::from_min_max(pos2(below.left(), below.bottom() - 12.0), below.max);
        let face = Rect::from_min_max(below.min, pos2(below.right(), value_row.top()));
        let response = ui.interact(self.rect, self.id(), Sense::click_and_drag());
        crate::focus::register(ui.ctx(), self.id(), bl_shell::Area::Detail, None);
        self.value_input(ui, &response, events);
        let c = &theme.colors;
        let painter = ui.painter();
        let centre = face.center();
        let radius = (face.height().min(face.width()) / 2.0 - 2.0).max(4.0);
        // The arc sweeps 300 degrees, from lower left to lower right.
        let (from, sweep) = (PI * 0.5 + PI / 6.0, PI * 5.0 / 3.0);
        let arc = |to: f32| -> Vec<egui::Pos2> {
            let steps = 32;
            (0..=steps)
                .map(|i| {
                    let a = from + sweep * to * i as f32 / steps as f32;
                    centre + vec2(a.cos(), a.sin()) * radius
                })
                .collect()
        };
        painter.add(Shape::line(arc(1.0), Stroke::new(2.0, c.control.0)));
        let n = self.descriptor.normalize(self.shown()) as f32;
        if n > 0.0 {
            painter.add(Shape::line(arc(n), Stroke::new(2.0, self.lit(theme))));
        }
        // Spec: modulation R7: the pointer stays at the base, and the ring
        // shows the way to the value it plays.
        if let Some(segment) = self
            .modulated
            .and_then(|(_, played)| ring_segment(n, self.descriptor.normalize(played) as f32))
        {
            paint_ring_segment(painter, theme, segment, |to| {
                let a = from + sweep * to;
                centre + vec2(a.cos(), a.sin()) * radius
            });
        }
        let a = from + sweep * n;
        painter.line_segment(
            [centre, centre + vec2(a.cos(), a.sin()) * radius],
            Stroke::new(1.5, c.text.0),
        );
        painter.text(
            value_row.center(),
            Align2::CENTER_CENTER,
            self.descriptor.format(self.shown()),
            theme.small_font(),
            c.text.0,
        );
        focus_ring(ui, theme, self.rect, &response);
        self.describe(ui, &response, Role::Slider);
    }

    /// A number box: a dark box with the value centred and a fill showing
    /// where it sits in its range.
    fn number(&self, ui: &mut Ui, theme: &Theme, events: &mut Vec<UiEvent>) {
        let below = self.label(ui, theme);
        let field = Rect::from_center_size(below.center(), vec2(below.width() - 4.0, 16.0));
        let response = ui.interact(field, self.id(), Sense::click_and_drag());
        crate::focus::register(ui.ctx(), self.id(), bl_shell::Area::Detail, None);
        self.value_input(ui, &response, events);
        let c = &theme.colors;
        let painter = ui.painter();
        painter.rect_filled(field, theme.sizes.corner, c.window.0);
        let n = self.descriptor.normalize(self.value) as f32;
        let fill_colour = if self.enabled {
            c.value_fill.0
        } else {
            highlight(theme, false)
        };
        let fill = Rect::from_min_size(field.min, vec2(field.width() * n, field.height()));
        painter.rect_filled(fill, theme.sizes.corner, fill_colour);
        painter.text(
            field.center(),
            Align2::CENTER_CENTER,
            self.descriptor.format(self.value),
            theme.small_font(),
            c.text.0,
        );
        focus_ring(ui, theme, field, &response);
        self.describe(ui, &response, Role::SpinButton);
    }

    /// A switch: a rectangular text button, lit while on.
    fn toggle(&self, ui: &mut Ui, theme: &Theme, events: &mut Vec<UiEvent>) {
        let below = self.label(ui, theme);
        let button = Rect::from_center_size(below.center(), vec2(below.width() - 4.0, 16.0));
        let response = ui.interact(button, self.id(), Sense::click());
        crate::focus::register(ui.ctx(), self.id(), bl_shell::Area::Detail, None);
        let on = self.value >= 0.5;
        if response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
            self.change(if on { 0.0 } else { 1.0 }, events);
        }
        let c = &theme.colors;
        let (face, ink) = if on {
            (self.lit(theme), c.accent_text.0)
        } else {
            (c.control.0, c.text.0)
        };
        ui.painter().rect_filled(button, theme.sizes.corner, face);
        ui.painter().text(
            button.center(),
            Align2::CENTER_CENTER,
            &self.descriptor.name,
            theme.small_font(),
            ink,
        );
        focus_ring(ui, theme, button, &response);
        let name = self.descriptor.name.clone();
        response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, &name));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Switch);
            node.set_label(self.descriptor.name.clone());
            node.set_value(self.descriptor.format(self.value));
        });
    }

    /// A chooser: a dark box with the value and a ▼, opening a menu of the
    /// choices.
    fn chooser(&self, ui: &mut Ui, theme: &Theme, events: &mut Vec<UiEvent>) {
        let below = self.label(ui, theme);
        let field = Rect::from_center_size(below.center(), vec2(below.width() - 4.0, 16.0));
        let response = ui.interact(field, self.id(), Sense::click());
        crate::focus::register(ui.ctx(), self.id(), bl_shell::Area::Detail, None);
        let c = &theme.colors;
        ui.painter()
            .rect_filled(field, theme.sizes.corner, c.window.0);
        ui.painter().text(
            pos2(field.left() + 3.0, field.center().y),
            Align2::LEFT_CENTER,
            self.descriptor.format(self.value),
            theme.small_font(),
            c.text.0,
        );
        ui.painter().text(
            pos2(field.right() - 3.0, field.center().y),
            Align2::RIGHT_CENTER,
            "▼",
            theme.small_font(),
            c.text_dim.0,
        );
        focus_ring(ui, theme, field, &response);
        if let ParamKind::Choice { choices } = &self.descriptor.kind {
            egui::Popup::menu(&response).show(|ui| {
                for (index, choice) in choices.iter().enumerate() {
                    let chosen = index as f64 == self.value;
                    if ui.selectable_label(chosen, choice).clicked() {
                        self.change(index as f64, events);
                        ui.close();
                    }
                }
            });
        }
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::ComboBox);
            node.set_label(self.descriptor.name.clone());
            node.set_value(self.descriptor.format(self.value));
        });
    }
}

/// The value `steps` key presses from `value`: one step of a stepped
/// parameter, or 1% of the travel.
fn step(param: &ParamDescriptor, value: f64, steps: i32) -> f64 {
    if param.is_stepped() {
        param.clamp(value + f64::from(steps))
    } else {
        param.denormalize(param.normalize(value) + f64::from(steps) * 0.01)
    }
}

fn focus_ring(ui: &Ui, theme: &Theme, rect: Rect, response: &Response) {
    if response.has_focus() && crate::focus::outlined(ui.ctx()) {
        ui.painter().rect_stroke(
            rect.expand(1.5),
            theme.sizes.corner,
            Stroke::new(1.0, theme.colors.focus.0),
            StrokeKind::Outside,
        );
    }
}
