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

use bl_model::device_layout::{PlacedPages, Size, Widget};
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

/// The ID of a page's tab or section button in a device's panel.
#[must_use]
pub fn page_button_id(device: DeviceId, node: &str, page: &str) -> Id {
    Id::new(("device-page", device, node.to_owned(), page.to_owned()))
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
    let (controls, cells, _) = place_with_pages(panel, expanded, body);
    (controls, cells)
}

/// The same, and each pages node's buttons placed in `body` (device
/// framework R41).
#[must_use]
pub fn place_with_pages(
    panel: &DevicePanel,
    expanded: bool,
    body: Rect,
) -> (ControlRects, CellRects, Vec<PlacedPages>) {
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
            .map(|c| (panel.drawn(c.param), c.widget, at(c.rect)))
            .collect(),
        placed
            .cells
            .iter()
            .map(|c| (c.name.clone(), at(c.rect), c.slot))
            .collect(),
        placed
            .pages
            .into_iter()
            .map(|mut pages| {
                for rect in &mut pages.buttons {
                    *rect = bl_model::device_layout::Rect::new(
                        body.min.x + rect.x,
                        body.min.y + rect.y,
                        rect.width,
                        rect.height,
                    );
                }
                pages
            })
            .collect(),
    )
}

/// Whether a modulated parameter has its control on page `page` of pages
/// node `node` (device framework R44): its tab's indicator is lit. `played`
/// is the panel's lookup of the parameters a clip modulates.
fn page_is_modulated(
    panel: &DevicePanel,
    node: &str,
    page: usize,
    played: &dyn Fn(ParamId) -> Option<(f64, f64)>,
) -> bool {
    panel
        .description
        .layout
        .pages()
        .into_iter()
        .find(|n| n.name == node)
        .and_then(|n| n.pages.get(page))
        .is_some_and(|page| {
            page.params()
                .into_iter()
                .any(|param| played(param).is_some())
        })
}

/// One tab or section button of a pages node: lit while its page shows,
/// with the modulation indicator at its top-right corner while `modulated`.
/// Returns whether it was chosen, by a click or Enter.
fn page_tab(
    ui: &mut Ui,
    theme: &Theme,
    (id, rect): (Id, Rect),
    (name, selected, modulated): (&str, bool, bool),
) -> bool {
    let response = ui.interact(rect, id, Sense::click());
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    let pressed =
        response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
    let c = &theme.colors;
    let face = if selected {
        c.accent.0
    } else if response.hovered() {
        c.control_hover.0
    } else {
        c.control.0
    };
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect_filled(rect.shrink(0.5), theme.sizes.corner, face);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        name,
        theme.small_font(),
        if selected { c.accent_text.0 } else { c.text.0 },
    );
    if modulated {
        painter.circle_filled(
            rect.right_top() + vec2(-4.0, 4.0),
            2.5,
            if selected {
                c.accent_text.0
            } else {
                c.accent.0
            },
        );
    }
    if response.has_focus() && crate::focus::outlined(ui.ctx()) {
        painter.rect_stroke(
            rect.expand(1.5),
            theme.sizes.corner,
            Stroke::new(1.0, c.focus.0),
            StrokeKind::Outside,
        );
    }
    crate::clip_panels::tab(ui, &response, name, selected);
    pressed
}

/// The tab row or shell strip of every pages node placed in the body, and
/// the title bar's tabs of those in that style (device framework R41, R44).
fn page_row(
    ui: &mut Ui,
    theme: &Theme,
    (frame, panel): (&DeviceFrame, &DevicePanel),
    (node, rects): (&str, &[Rect]),
    played: &dyn Fn(ParamId) -> Option<(f64, f64)>,
    events: &mut Vec<UiEvent>,
) {
    let Some(frame_of) = panel.pages.iter().find(|p| p.node == node) else {
        return;
    };
    let ids: Vec<Id> = frame_of
        .pages
        .iter()
        .map(|page| page_button_id(frame.id, node, page))
        .collect();
    if let Some(span) = rects.iter().copied().reduce(|a, b| a.union(b)) {
        crate::clip_panels::tab_list(ui, Id::new(("device-pages", frame.id, node)), span, node);
    }
    for (index, (page, rect)) in frame_of.pages.iter().zip(rects).enumerate() {
        let selected = *page == frame_of.shown;
        let modulated = page_is_modulated(panel, node, index, played);
        if page_tab(ui, theme, (ids[index], *rect), (page, selected, modulated)) && !selected {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::SET_PAGE,
                json!({ "device": frame.id, "node": node, "page": page }),
            )));
        }
    }
    crate::clip_panels::selector_keys(ui, &ids);
}

/// The tabs of the pages nodes in the title-bar style, in a run at the right
/// of the title (L-DEV-017), except a multisampler's, which its own title-bar
/// tabs draw (device framework R41; multisampler R75).
pub fn title_tabs(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    title: Rect,
    played: &dyn Fn(ParamId) -> Option<(f64, f64)>,
    events: &mut Vec<UiEvent>,
) {
    let Some(panel) = frame
        .panel
        .as_ref()
        .filter(|_| frame.multisampler.is_none())
    else {
        return;
    };
    for node in panel
        .pages
        .iter()
        .filter(|p| p.style == bl_model::device_layout::PageStyle::TitleBar)
    {
        let width = |name: &str| name.chars().count() as f32 * 6.0 + 16.0;
        let total: f32 = node.pages.iter().map(|p| width(p) + 2.0).sum();
        let mut x = (title.right() - total).max(title.left() + 70.0);
        let rects: Vec<Rect> = node
            .pages
            .iter()
            .map(|page| {
                let rect = Rect::from_min_size(
                    pos2(x, title.top() + 1.0),
                    vec2(width(page), title.height() - 2.0),
                );
                x += width(page) + 2.0;
                rect
            })
            .collect();
        page_row(
            ui,
            theme,
            (frame, panel),
            (&node.node, &rects),
            played,
            events,
        );
    }
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
    let (controls, cells, pages) = place_with_pages(panel, frame.expanded, body);
    for placed in pages.iter().filter(|p| !p.buttons.is_empty()) {
        let rects: Vec<Rect> = placed
            .buttons
            .iter()
            .map(|r| Rect::from_min_size(pos2(r.x, r.y), vec2(r.width, r.height)))
            .collect();
        page_row(
            ui,
            theme,
            (frame, panel),
            (&placed.name, &rects),
            played,
            events,
        );
    }
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
        // Spec: corpus R1. Corpus's X–Y control is its display cell.
        if matches!(&frame.kind, bl_model::DeviceKind::Native { device_type }
            if device_type == bl_devices::corpus::CORPUS)
            && name == bl_devices::corpus::XY_CELL
        {
            corpus_xy(ui, theme, frame, (panel, display), (id, rect), events);
            continue;
        }
        let _ = ui.interact(rect, id, Sense::hover());
        // Spec: drum-synths R6. A click in the upper half of a Drum Synth's
        // display plays a hit.
        if bl_devices::drum_synth::is_drum_synth(&frame.kind)
            && name == bl_devices::drum_synth::ENVELOPE
        {
            let upper = Rect::from_min_max(rect.min, pos2(rect.right(), rect.center().y));
            let preview = id.with("preview");
            let response = ui.interact(upper, preview, Sense::click());
            crate::focus::register(ui.ctx(), preview, bl_shell::Area::Detail, None);
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Preview Hit"));
            if response.clicked() {
                events.push(UiEvent::Invoke(Invocation::new(
                    "device.preview_hit",
                    json!({ "device": frame.id }),
                )));
            }
        }
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
                    spectrograms: Vec::new(),
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
            // Spec: roar R31. Roar's source traces have cells of their own
            // on their pages; the shaper's cell leaves them out.
            Some(d) if index == 0 && has_drawing(d) => {
                let roar = frame.kind
                    == bl_model::DeviceKind::Native {
                        device_type: bl_devices::roar::ROAR.to_owned(),
                    };
                own = DeviceDisplay {
                    series: d
                        .series
                        .iter()
                        .filter(|s| !(roar && bl_devices::roar::SOURCES.contains(&s.name.as_str())))
                        .cloned()
                        .collect(),
                    ..d.clone()
                };
                Some(&own)
            }
            _ => None,
        };
        if let Some(display) = shown {
            // The drag goes under the display, so its view buttons stay on
            // top and take their clicks.
            display_drag(ui, frame, panel, display, rect, id, events);
            crate::device_display::draw(ui, theme, rect, id, frame.enabled, display, panel, events);
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
        // Spec: meld R3, R5. A relabelled parameter shows its label's name,
        // and its number in the label's unit.
        let read = |id| panel.read(id);
        let descriptor = panel
            .description
            .described(&read, &panel.description.params[index]);
        let value = panel.value(index);
        let relabelled_text = panel
            .description
            .label(&read, param)
            .is_some_and(|label| label.unit.is_some())
            .then(|| panel.description.text(&read, &descriptor, value));
        let control = Control {
            device: frame.id,
            enabled: frame.enabled,
            descriptor: &descriptor,
            value,
            modulated: played(param),
            routes: panel
                .matrix_routes
                .iter()
                .find(|routes| routes.param == param)
                .map_or(&[][..], |routes| &routes.sources),
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
            .flatten()
            .or(relabelled_text),
            automation: menu(param),
            coarse: if frame.kind
                == (bl_model::DeviceKind::Native {
                    device_type: bl_devices::roar::ROAR.to_owned(),
                })
                && param == bl_devices::roar::FB_NOTE
            {
                12
            } else {
                1
            },
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
        // Spec: map-modes R3, R11; macros R35. MIDI Map Mode and Key Map Mode
        // tint the parameters they can map, a toggle or a choice by its own
        // kind; one a macro holds has no overlay.
        if crate::mapping::held(ui.ctx(), frame.id, param).is_none() {
            crate::mapping::remote_overlay(
                ui,
                theme,
                rect,
                param_control_id(frame.id, param),
                bl_model::RemoteTarget::Device {
                    device: frame.id,
                    param,
                },
                matches!(widget, Widget::Knob | Widget::Number),
                events,
            );
        }
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

/// Spec: roar R12. A vertical drag on a display that names parameters to drag
/// (the description's `display_drag`, one for each view) changes the shown
/// view's, as a drag on its knob does: one undo step named for the parameter.
fn display_drag(
    ui: &mut Ui,
    frame: &DeviceFrame,
    panel: &DevicePanel,
    display: &DeviceDisplay,
    rect: Rect,
    id: Id,
    events: &mut Vec<UiEvent>,
) {
    let description = &panel.description;
    let view = description
        .views
        .iter()
        .position(|v| Some(v.as_str()) == display.view.as_deref())
        .unwrap_or(0);
    let Some(param) = description.display_drag.get(view) else {
        return;
    };
    let Some(index) = description.params.iter().position(|p| p.id == *param) else {
        return;
    };
    let control = Control {
        device: frame.id,
        enabled: frame.enabled,
        descriptor: &description.params[index],
        value: panel.value(index),
        modulated: None,
        routes: &[],
        rect,
        stops: &[],
        knob: None,
        configured: &[],
        text: None,
        automation: None,
        coarse: 1,
    };
    let response = ui.interact(rect, id.with("drag"), Sense::click_and_drag());
    control.value_input(ui, &response, events);
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

/// Corpus's X–Y control (the Corpus spec, R1): Decay across and Material up,
/// or Radius up for a pipe and a tube, a dot where they stand and the
/// limiter's lamp in a corner. A press or a drag sets both at the pointer, as
/// one undo step.
fn corpus_xy(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    (panel, display): (&DevicePanel, Option<&DeviceDisplay>),
    (id, rect): (Id, Rect),
    events: &mut Vec<UiEvent>,
) {
    use bl_devices::corpus::{DECAY, LAMP_DB, LIMITER, TYPE, xy_vertical};
    let find = |param: ParamId| panel.description.params.iter().position(|p| p.id == param);
    let (Some(x_at), Some(type_at)) = (find(DECAY), find(TYPE)) else {
        return;
    };
    let Some(y_at) = find(xy_vertical(panel.value(type_at) as usize)) else {
        return;
    };
    let (x, y) = (
        &panel.description.params[x_at],
        &panel.description.params[y_at],
    );
    let response = ui.interact(rect, id, Sense::click_and_drag());
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    let set = |param: &ParamDescriptor, value: f64| {
        UiEvent::Invoke(Invocation::new(
            "device.set_param",
            json!({ "device": frame.id, "param": param.id, "value": value }),
        ))
    };
    let at_pointer = |events: &mut Vec<UiEvent>| {
        let Some(pointer) = response.interact_pointer_pos() else {
            return;
        };
        let across = f64::from(((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
        let up = f64::from(((rect.bottom() - pointer.y) / rect.height()).clamp(0.0, 1.0));
        events.push(set(x, x.denormalize(across)));
        events.push(set(y, y.denormalize(up)));
    };
    let gesture =
        || UiEvent::BeginNamedGesture(format!("Change \"{}\" and \"{}\"", x.name, y.name));
    if response.drag_started() {
        events.push(gesture());
    }
    if response.dragged() {
        at_pointer(events);
    }
    if response.drag_stopped() {
        events.push(UiEvent::EndGesture);
    }
    if response.clicked() {
        events.push(gesture());
        at_pointer(events);
        events.push(UiEvent::EndGesture);
    }
    let ink = highlight(theme, frame.enabled);
    let painter = ui.painter_at(rect);
    let dot = pos2(
        rect.left() + rect.width() * x.normalize(panel.value(x_at)) as f32,
        rect.bottom() - rect.height() * y.normalize(panel.value(y_at)) as f32,
    );
    let line = Stroke::new(1.0, ink.gamma_multiply(0.4));
    painter.hline(rect.x_range(), dot.y, line);
    painter.vline(dot.x, rect.y_range(), line);
    painter.circle_filled(dot, 4.0, ink);
    let lamp = display
        .and_then(|d| d.reading(LIMITER))
        .is_some_and(|reading| reading.value < LAMP_DB);
    painter.circle(
        pos2(rect.right() - 7.0, rect.top() + 7.0),
        3.0,
        if lamp { ink } else { Color32::TRANSPARENT },
        Stroke::new(1.0, theme.colors.separator.0),
    );
    focus_ring(ui, theme, rect, &response);
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Image);
        node.set_label(bl_devices::corpus::XY_CELL.to_owned());
        node.set_value(format!(
            "{} {}, {} {}{}",
            x.name,
            x.format(panel.value(x_at)),
            y.name,
            y.format(panel.value(y_at)),
            if lamp { ", limiter on" } else { "" }
        ));
    });
}

/// Whether a display has anything to draw.
fn has_drawing(display: &DeviceDisplay) -> bool {
    !(display.series.is_empty() && display.readings.is_empty() && display.spectrograms.is_empty())
}

/// One control being drawn.
struct Control<'a> {
    device: DeviceId,
    enabled: bool,
    descriptor: &'a ParamDescriptor,
    value: f64,
    /// Its base and what it plays while a clip modulates it.
    modulated: Option<(f64, f64)>,
    /// The sources an instrument's matrix routes into it, read beside its
    /// value (instrument modulation R12).
    routes: &'a [String],
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
    /// How many steps Shift with Up or Down moves it: 12 for Roar's
    /// Feedback Note, an octave (the roar spec, R26), else 1.
    coarse: i32,
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
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } = event
                    {
                        // Spec: roar R26. Shift moves a coarse control by
                        // its coarse step.
                        let by = if modifiers.shift { self.coarse } else { 1 };
                        match key {
                            Key::ArrowUp => steps += by,
                            Key::ArrowDown => steps -= by,
                            Key::Delete | Key::Backspace => reset = true,
                            _ => {}
                        }
                    }
                }
            });
        }
        // Spec: view-options R11 to R13. A turn of the wheel moves it as the
        // keys do, a tenth of a step with Shift (a stepped value has no
        // finer step), in the run's one undo step.
        if let Some(turn) = crate::wheel::input(response, || gesture_name(d)) {
            let value = if turn.fine && !d.is_stepped() {
                d.denormalize(d.normalize(self.value) + f64::from(turn.steps) * 0.001)
            } else {
                step(d, self.value, turn.steps)
            };
            let value = d.clamp(value);
            crate::mixer::show_readout(ui.ctx(), readout(d, value));
            if value != self.value {
                events.push(UiEvent::Invoke(self.set(value)));
            }
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
        let text = crate::widgets::routed_text(
            modulated_text(
                self.text.clone().unwrap_or_else(|| d.format(self.shown())),
                self.modulated.map(|(_, played)| d.format(played)),
            ),
            self.routes,
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
pub(crate) fn step(param: &ParamDescriptor, value: f64, steps: i32) -> f64 {
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
