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
use bl_shell::device_commands as ids;
use bl_shell::device_commands::{ConfiguredParam, DeviceFrame, DevicePanel};
use bl_shell::plugin_commands;
use egui::accesskit::{Action, Role};
use egui::{
    Align2, Color32, Event, Id, Key, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::param_menu::ParamMenu;
use crate::theme::Theme;
use crate::widgets::{modulated_text, paint_ring_segment, ring_segment};

/// What the panel asks of the device view for each parameter: its base and
/// played value while a clip modulates it, and its context menu's automation
/// items (the automation spec, R29).
pub type ParamLookups<'a> = (
    &'a dyn Fn(ParamId) -> Option<(f64, f64)>,
    &'a dyn Fn(ParamId) -> Option<ParamMenu>,
);

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

/// Each display cell's name, its place in a panel, and the sample slot it
/// stands for if it does.
pub type CellRects = Vec<(String, Rect, Option<u32>)>;

/// Where each control and display cell sits in a body at `body`: the
/// layout placed at the body's size ([`DeviceLayout::place_for`]) with the
/// panel's selected sample slot showing, so docked it sits at its docked
/// size, and a floating window (WP-2.5b) passes a larger body.
///
/// [`DeviceLayout::place_for`]: bl_model::DeviceLayout::place_for
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
        .place_for(Size::new(body.width(), body.height()), panel.selected_slot);
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
            .map(|c| (c.name.clone(), at(c.rect), c.slot))
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
/// value of a parameter while a clip modulates it, and `menu` the automation
/// items of its context menu (the automation spec, R29).
#[expect(clippy::too_many_arguments, reason = "the panel's inputs")]
pub fn show(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    display: Option<&DeviceDisplay>,
    body: Rect,
    (played, menu): ParamLookups<'_>,
    renaming: &mut Option<crate::articulation_map::Renaming>,
    events: &mut Vec<UiEvent>,
) {
    let Some(panel) = frame.panel.as_ref() else {
        return;
    };
    let c = &theme.colors;
    let (controls, cells) = place_shown(panel, frame.expanded, body);
    for (index, (name, rect, slot)) in cells.into_iter().enumerate() {
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
        if let Some(slot) = slot {
            slot_cell(ui, theme, frame, panel, (id, &name, slot, rect), events);
            continue;
        }
        let _ = ui.interact(rect, id, Sense::hover());
        // Spec: modulation R15, R23. A modulator's cells show its slots'
        // mappings and, for the Shaper, its shape.
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
        // A cell named like one of the display's series draws only that
        // series (the Expression Control's tabs); otherwise the first cell draws it all.
        let own;
        let shown = match display {
            Some(d) if d.series.iter().any(|s| s.name == name) => {
                own = DeviceDisplay {
                    series: d
                        .series
                        .iter()
                        .filter(|s| s.name == name)
                        .cloned()
                        .collect(),
                    markers: Vec::new(),
                    readings: Vec::new(),
                    ..d.clone()
                };
                Some(&own)
            }
            Some(d) if index == 0 && has_drawing(d) => Some(d),
            _ => None,
        };
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
        // Spec: articulation-map R15. The Articulation Map's list shows
        // every articulation, the current one lit.
        let list = crate::articulation_map::is_list(&name);
        let current = display
            .and_then(|display| display.reading("Articulation"))
            .map_or(0, |reading| reading.value.max(1.0) as usize - 1);
        if list && !panel.articulations.is_empty() {
            crate::articulation_map::cell(ui, theme, frame, current, rect, renaming, events);
        }
        // Spec: articulation-map R15. The list reads as its rows.
        let rows = list.then(|| crate::articulation_map::list_text(&panel.articulations, current));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Image);
            node.set_label(name);
            if let Some(rows) = rows {
                node.set_value(rows);
            } else if let Some(display) = shown {
                node.set_value(display.text());
            }
        });
    }
    for (at, (param, widget, rect)) in controls.into_iter().enumerate() {
        // A plug-in panel's controls are its knobs, in the panel's own order
        // (WP-5.40), so a control's place is its knob; any other device's
        // controls are found by their parameter.
        let index = if panel.plugin {
            at
        } else {
            let Some(found) = panel.description.params.iter().position(|p| p.id == param) else {
                continue;
            };
            found
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
            stops: if frame.multisampler.is_some()
                && param == bl_devices::multisampler::FILTER_MORPH
            {
                &bl_devices::multisampler::MORPH_STOPS
            } else {
                &[]
            },
            // Spec: clap-hosting R5 (WP-5.40). A plug-in panel's knobs offer
            // the configured parameters on a right-click.
            knob: panel
                .plugin
                .then(|| panel.knobs.get(index).copied().unwrap_or(index)),
            configured: &panel.configured,
            // Spec: articulation-map R6. The Articulation control reads
            // the articulation's name.
            text: (param == bl_devices::articulation_map::ARTICULATION
                && !panel.articulations.is_empty())
            .then(|| {
                display
                    .and_then(|display| display.reading("Name"))
                    .map(|reading| reading.text.clone())
            })
            .flatten(),
            automation: menu(param),
        };
        // Spec: scales R16. A parameter the device takes from its current
        // scale is drawn disabled.
        let locked = panel.locked.contains(&param);
        let mut draw = |ui: &mut Ui| match widget {
            Widget::Knob => control.knob(ui, theme, events),
            Widget::Number => control.number(ui, theme, events),
            Widget::Toggle => control.toggle(ui, theme, events),
            Widget::Chooser => control.chooser(ui, theme, events),
        };
        if locked {
            ui.add_enabled_ui(false, draw);
            ui.painter().rect_filled(
                rect,
                theme.sizes.corner,
                theme.colors.window.0.gamma_multiply(0.55),
            );
        } else {
            draw(ui);
        }
        // Spec: automation R29. A click shows the parameter's envelope while
        // Automation Mode is on, and its context menu holds the automation
        // items: after the control's own where it has some.
        if let Some(menu) = &control.automation {
            if control.stops.is_empty() && control.knob.is_none() {
                menu.overlay(ui, rect, control.id(), events);
            } else {
                menu.click(ui, rect, events);
            }
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
        // Spec: macros R3, R5. While a rack's Map mode is on, the parameters
        // it can map are tinted, and a click selects one for a macro's Map
        // button.
        crate::mapping::macro_overlay(ui, theme, rect, frame.id, param);
        // Spec: macros R11. A parameter a macro holds is drawn disabled, and
        // the Status Bar names the macro while the pointer is over it.
        if let Some(name) = crate::mapping::held(ui.ctx(), frame.id, param) {
            let hovered = ui
                .ctx()
                .pointer_hover_pos()
                .is_some_and(|at| rect.contains(at));
            if hovered {
                crate::mapping::set_hover_hold(ui.ctx(), Some(name.clone()));
            }
            ui.painter().rect_filled(
                rect,
                theme.sizes.corner,
                theme.colors.window.0.gamma_multiply(0.55),
            );
            ui.ctx()
                .accesskit_node_builder(param_control_id(frame.id, param), |node| {
                    node.set_description(format!("Held by macro \"{name}\""));
                });
        }
    }
}

/// What a control's name above it reads: a sample slot's parameter drops the
/// slot's number (`Slot 3 Start` reads `Start`), since the panel shows one
/// slot's at a time.
fn short_name(name: &str) -> &str {
    name.strip_prefix("Slot ")
        .and_then(|rest| rest.split_once(' '))
        .filter(|(number, _)| number.chars().all(|c| c.is_ascii_digit()))
        .map_or(name, |(_, rest)| rest)
}

/// A display cell standing for a sample slot (WP-5.17): its number and key,
/// whether it holds a sample, and a frame in the accent while its controls
/// are the ones the panel shows. Clicking it, or Enter, selects the slot;
/// Delete clears its sample. A sample dropped on it loads into it
/// ([`crate::device_view`]).
fn slot_cell(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    panel: &DevicePanel,
    (id, name, slot, rect): (Id, &str, u32, Rect),
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let response = ui.interact(rect, id, Sense::click());
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    if response.clicked() {
        response.request_focus();
    }
    // While a slot's cell has the keyboard, Delete clears its sample
    // rather than deleting the device (S-DEV-006).
    if response.gained_focus() {
        events.push(UiEvent::FocusValue(true));
    }
    if response.lost_focus() {
        events.push(UiEvent::FocusValue(false));
    }
    let selected = panel.selected_slot == slot;
    let loaded = panel
        .loaded_slots
        .get(slot as usize)
        .copied()
        .unwrap_or(false);
    if (response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))))
        && !selected
    {
        events.push(UiEvent::Invoke(Invocation::new(
            ids::SELECT_SLOT,
            json!({ "device": frame.id, "slot": slot }),
        )));
    }
    if response.has_focus()
        && loaded
        && ui.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace))
    {
        events.push(UiEvent::Invoke(Invocation::new(
            "device.set_sample",
            json!({ "device": frame.id, "media": null, "slot": slot }),
        )));
    }
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_stroke(
            rect.shrink(1.0),
            0.0,
            Stroke::new(1.5, highlight(theme, frame.enabled)),
            StrokeKind::Inside,
        );
    }
    let key = panel
        .slot_keys
        .get(slot as usize)
        .cloned()
        .unwrap_or_default();
    painter.text(
        rect.center_top() + vec2(0.0, 4.0),
        Align2::CENTER_TOP,
        format!("{}", slot + 1),
        theme.small_font(),
        c.text.0,
    );
    painter.text(
        rect.center_bottom() - vec2(0.0, 4.0),
        Align2::CENTER_BOTTOM,
        &key,
        theme.small_font(),
        c.text_dim.0,
    );
    if loaded {
        // A bar along the cell's foot: this slot holds a sample.
        let bar = Rect::from_min_size(
            pos2(rect.left() + 4.0, rect.bottom() - 3.0),
            vec2(rect.width() - 8.0, 2.0),
        );
        painter.rect_filled(bar, 0.0, highlight(theme, frame.enabled));
    }
    focus_ring(ui, theme, rect, &response);
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Button);
        node.set_label(name.to_owned());
        node.set_description(format!(
            "plays {key}, {}{}",
            if loaded { "holds a sample" } else { "empty" },
            if selected { ", selected" } else { "" }
        ));
    });
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
    /// For a knob whose context menu snaps it to a stop: the stops' names
    /// and values (the multisampler's Morph, spec R37).
    stops: &'static [(&'static str, f64)],
    /// A plug-in panel knob's index, 0 for the first: its right-click menu
    /// remaps that knob (clap-hosting R5, WP-5.40). None for any other
    /// control.
    knob: Option<usize>,
    /// The parameters a plug-in device's panel is configured to show, in
    /// order: what a knob's right-click menu offers. Empty otherwise.
    configured: &'a [ConfiguredParam],
    /// The value the control shows, when it reads something else than its
    /// number (the Articulation Map's Articulation control reads the
    /// articulation's name).
    text: Option<String>,
    /// The automation items of its context menu (the automation spec, R29).
    automation: Option<ParamMenu>,
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
            short_name(&self.descriptor.name),
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
            self.text.clone().unwrap_or_else(|| d.format(self.shown())),
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
        if !self.stops.is_empty() {
            response.context_menu(|ui| {
                for (name, value) in self.stops {
                    if ui.button(*name).clicked() {
                        self.change(*value, events);
                        ui.close();
                    }
                }
                if let Some(menu) = &self.automation {
                    menu.items(ui, true, events);
                }
            });
        }
        if let Some(knob) = self.knob {
            // Spec: clap-hosting R5 (WP-5.40, manual 24.1.2). A person remaps
            // a knob by right-clicking it and picking one of the device's
            // configured parameters, or puts it back to following the list.
            let remapped = self
                .configured
                .get(knob)
                .is_none_or(|entry| entry.param != self.descriptor.id);
            response.context_menu(|ui| {
                if remapped && ui.button("Follow the List").clicked() {
                    events.push(UiEvent::Invoke(Invocation::new(
                        plugin_commands::SET_PANEL_PARAM,
                        json!({ "device": self.device, "index": knob, "param": null }),
                    )));
                    ui.close();
                }
                if remapped {
                    ui.separator();
                }
                for entry in self.configured {
                    if ui.button(&entry.name).clicked() {
                        events.push(UiEvent::Invoke(Invocation::new(
                            plugin_commands::SET_PANEL_PARAM,
                            json!({ "device": self.device, "index": knob, "param": entry.param }),
                        )));
                        ui.close();
                    }
                }
                if let Some(menu) = &self.automation {
                    menu.items(ui, true, events);
                }
            });
        }
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
            self.text
                .clone()
                .unwrap_or_else(|| self.descriptor.format(self.value)),
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
            short_name(&self.descriptor.name),
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

pub(crate) fn focus_ring(ui: &Ui, theme: &Theme, rect: Rect, response: &Response) {
    if response.has_focus() && crate::focus::outlined(ui.ctx()) {
        ui.painter().rect_stroke(
            rect.expand(1.5),
            theme.sizes.corner,
            Stroke::new(1.0, theme.colors.focus.0),
            StrokeKind::Outside,
        );
    }
}
