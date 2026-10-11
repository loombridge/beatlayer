//! The shell's controls: drawn in Beatlayer's own style (never Ableton's
//! icons), each with a role, a name, and a value for assistive technology.

use egui::accesskit::{self, Role};
use egui::{
    Align2, Color32, Id, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo,
    WidgetType, pos2, vec2,
};

use crate::layout::{Element, Kind, Placement};
use crate::theme::Theme;

/// What an element shows now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    /// Text drawn in the element, if it isn't an icon.
    pub text: Option<String>,
    /// For a switch: whether it's on.
    pub on: bool,
    /// The value assistive technology reads, such as `120.00` or `48 kHz`.
    pub value: Option<String>,
    /// A numeric value, for value fields and meters.
    pub number: Option<f64>,
    /// More for assistive technology to read, such as the whole position a
    /// bar field is part of.
    pub description: Option<String>,
    /// Whether a value is being typed into the element, which it draws as
    /// an entry.
    pub entry: bool,
}

/// Draws an element of the Control Bar or Status Bar at `rect` and returns
/// its response. A pending element (one a later package brings to life) is
/// drawn quieter, can't be clicked or focused, and tells assistive
/// technology which package brings it. A live value field also takes drags
/// (I-DRAG-022), and a screen reader's increment and decrement.
pub fn element(
    ui: &mut Ui,
    theme: &Theme,
    placement: &Placement,
    rect: Rect,
    look: &Look,
) -> Response {
    let live = placement.pending.is_none();
    let sense = match placement.kind {
        _ if !live => Sense::hover(),
        Kind::Indicator => Sense::hover(),
        Kind::Value => Sense::click_and_drag(),
        _ => Sense::click(),
    };
    let id = Id::new(("element", placement.element));
    let response = ui.interact(rect, id, sense);
    let response = match placement.pending {
        Some(package) => response.on_hover_text(format!(
            "{}. Not built yet: arrives with {package}.",
            placement.name
        )),
        None => response.on_hover_text(placement.name),
    };
    paint(ui, theme, placement, rect, look, &response);
    describe(ui, placement, look, &response);
    response
}

fn paint(
    ui: &Ui,
    theme: &Theme,
    placement: &Placement,
    rect: Rect,
    look: &Look,
    response: &Response,
) {
    let c = &theme.colors;
    let live = placement.pending.is_none();
    let painter = ui.painter_at(rect.expand(3.0));
    let corner = theme.sizes.corner;
    let boxed = !matches!(placement.kind, Kind::MenuArrow | Kind::Indicator)
        && placement.element != Element::Message;
    let face = if look.on && live {
        c.accent.0
    } else if look.entry || response.is_pointer_button_down_on() {
        c.control_pressed.0
    } else if response.hovered() && live {
        c.control_hover.0
    } else {
        c.control.0
    };
    if boxed {
        let face = if live {
            face
        } else {
            face.gamma_multiply(0.55)
        };
        painter.rect_filled(rect, corner, face);
    }
    let ink = if !live {
        c.pending.0
    } else if look.on {
        c.accent_text.0
    } else {
        c.text.0
    };
    if let Some(text) = &look.text {
        let (anchor, pos) = match placement.element {
            Element::Message => (Align2::LEFT_CENTER, rect.left_center()),
            _ => (Align2::CENTER_CENTER, rect.center()),
        };
        let font = if rect.height() < 18.0 {
            theme.small_font()
        } else {
            theme.font()
        };
        let ink = if placement.element == Element::Message && look.on {
            c.error_text.0
        } else {
            ink
        };
        painter.text(pos, anchor, text, font, ink);
    } else {
        icon(&painter, placement.element, rect, ink, theme);
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            corner,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Outside,
        );
    }
}

/// Beatlayer's own icons: plain shapes.
fn icon(painter: &egui::Painter, element: Element, rect: Rect, ink: Color32, theme: &Theme) {
    let c = rect.center();
    let s = rect.height().min(rect.width()) * 0.5;
    let stroke = Stroke::new(1.2, ink);
    let triangle =
        |a: Pos2, b: Pos2, d: Pos2| Shape::convex_polygon(vec![a, b, d], ink, Stroke::NONE);
    match element {
        Element::BrowserToggle => {
            let r = Rect::from_center_size(c, vec2(s * 1.5, s * 1.2));
            painter.rect_stroke(r, 1.0, stroke, StrokeKind::Inside);
            painter.line_segment(
                [
                    pos2(r.left() + r.width() * 0.35, r.top()),
                    pos2(r.left() + r.width() * 0.35, r.bottom()),
                ],
                stroke,
            );
        }
        Element::BrowserMenu | Element::MetronomeMenu | Element::CpuMenu | Element::MixerMenu => {
            painter.add(triangle(
                pos2(c.x - 3.0, c.y - 1.5),
                pos2(c.x + 3.0, c.y - 1.5),
                pos2(c.x, c.y + 2.0),
            ));
        }
        Element::NudgeDown | Element::ClipViewToggle | Element::DeviceViewToggle => {
            painter.add(triangle(
                pos2(c.x + 2.5, c.y - 4.0),
                pos2(c.x + 2.5, c.y + 4.0),
                pos2(c.x - 3.0, c.y),
            ));
        }
        Element::NudgeUp | Element::Play | Element::Follow => {
            painter.add(triangle(
                pos2(c.x - 3.0, c.y - 4.5),
                pos2(c.x - 3.0, c.y + 4.5),
                pos2(c.x + 4.5, c.y),
            ));
            if element == Element::Follow {
                painter.line_segment(
                    [pos2(c.x - 6.0, c.y - 4.5), pos2(c.x - 6.0, c.y + 4.5)],
                    stroke,
                );
            }
        }
        Element::Stop => {
            painter.rect_filled(Rect::from_center_size(c, vec2(8.0, 8.0)), 0.0, ink);
        }
        Element::Record => {
            painter.circle_filled(
                c,
                4.5,
                theme
                    .colors
                    .record
                    .0
                    .gamma_multiply(if ink == theme.colors.pending.0 {
                        0.5
                    } else {
                        1.0
                    }),
            );
        }
        Element::SessionRecord => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(10.0, 10.0)),
                1.0,
                stroke,
                StrokeKind::Inside,
            );
            painter.circle_filled(c, 2.5, ink);
        }
        Element::AutomationArm => {
            painter.circle_stroke(c, 4.0, stroke);
            painter.circle_filled(c, 1.5, ink);
        }
        Element::ReEnableAutomation => {
            painter.line_segment(
                [pos2(c.x + 4.0, c.y + 3.0), pos2(c.x + 4.0, c.y - 3.0)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x + 4.0, c.y - 3.0), pos2(c.x - 3.0, c.y - 3.0)],
                stroke,
            );
            painter.add(triangle(
                pos2(c.x - 3.0, c.y - 6.0),
                pos2(c.x - 3.0, c.y),
                pos2(c.x - 6.0, c.y - 3.0),
            ));
        }
        Element::CaptureMidi => {
            for dy in [-3.0, 0.0, 3.0] {
                painter.line_segment(
                    [pos2(c.x - 4.0, c.y + dy), pos2(c.x + 4.0, c.y + dy)],
                    stroke,
                );
            }
        }
        Element::Metronome => {
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(c.x - 4.0, c.y + 5.0),
                    pos2(c.x + 4.0, c.y + 5.0),
                    pos2(c.x + 1.5, c.y - 5.0),
                    pos2(c.x - 1.5, c.y - 5.0),
                ],
                Color32::TRANSPARENT,
                stroke,
            ));
            painter.line_segment([pos2(c.x, c.y + 3.0), pos2(c.x + 3.0, c.y - 4.0)], stroke);
        }
        Element::Loop => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(12.0, 7.0)),
                3.0,
                stroke,
                StrokeKind::Inside,
            );
        }
        Element::DrawMode => {
            painter.line_segment(
                [pos2(c.x - 4.0, c.y + 4.0), pos2(c.x + 4.0, c.y - 4.0)],
                Stroke::new(2.0, ink),
            );
        }
        Element::ComputerKeyboard => {
            let r = Rect::from_center_size(c, vec2(12.0, 7.0));
            painter.rect_stroke(r, 1.0, stroke, StrokeKind::Inside);
            for i in 1..4 {
                let x = r.left() + r.width() * i as f32 / 4.0;
                painter.line_segment([pos2(x, r.top()), pos2(x, r.center().y)], stroke);
            }
        }
        Element::KeyIndicators | Element::MidiIndicators | Element::Overload => {
            let lamp = Rect::from_center_size(
                pos2(c.x, c.y - 2.5),
                vec2(rect.width().min(8.0) - 2.0, 3.0),
            );
            painter.rect_filled(lamp, 1.0, ink.gamma_multiply(0.6));
            painter.rect_filled(lamp.translate(vec2(0.0, 5.0)), 1.0, ink.gamma_multiply(0.6));
        }
        Element::SessionView => {
            for dx in [-3.5, 0.0, 3.5] {
                for dy in [-3.5, 0.0, 3.5] {
                    painter.rect_filled(
                        Rect::from_center_size(pos2(c.x + dx, c.y + dy), vec2(2.5, 2.5)),
                        0.0,
                        ink,
                    );
                }
            }
        }
        Element::ArrangementView => {
            for (dy, w) in [(-3.5, 9.0), (0.0, 6.0), (3.5, 8.0)] {
                painter.line_segment(
                    [pos2(c.x - 5.0, c.y + dy), pos2(c.x - 5.0 + w, c.y + dy)],
                    Stroke::new(2.0, ink),
                );
            }
        }
        Element::InfoToggle => {
            painter.text(c, Align2::CENTER_CENTER, "i", theme.small_font(), ink);
        }
        Element::PunchIn => {
            painter.text(c, Align2::CENTER_CENTER, "[", theme.font(), ink);
        }
        Element::PunchOut => {
            painter.text(c, Align2::CENTER_CENTER, "]", theme.font(), ink);
        }
        Element::Overdub => {
            painter.text(c, Align2::CENTER_CENTER, "+", theme.font(), ink);
        }
        Element::MixerToggle => {
            for dx in [-4.0, 0.0, 4.0] {
                painter.line_segment(
                    [pos2(c.x + dx, c.y - 4.0), pos2(c.x + dx, c.y + 4.0)],
                    stroke,
                );
            }
        }
        _ => {}
    }
}

/// Tells assistive technology what the element is, what it's called, its
/// value, and, for a pending element, which package brings it.
fn describe(ui: &Ui, placement: &Placement, look: &Look, response: &Response) {
    let live = placement.pending.is_none();
    let name = placement.name;
    response.widget_info(|| match placement.kind {
        Kind::Switch => WidgetInfo::selected(WidgetType::Button, live, look.on, name),
        Kind::Value => WidgetInfo {
            label: Some(name.to_owned()),
            ..WidgetInfo::drag_value(live, look.number.unwrap_or_default())
        },
        Kind::Chooser => WidgetInfo::labeled(WidgetType::ComboBox, live, name),
        Kind::Button | Kind::MenuArrow | Kind::Display => {
            WidgetInfo::labeled(WidgetType::Button, live, name)
        }
        Kind::Meter => WidgetInfo::labeled(WidgetType::ProgressIndicator, live, name),
        Kind::Indicator => WidgetInfo::labeled(WidgetType::Other, live, name),
    });
    ui.ctx().accesskit_node_builder(response.id, |node| {
        match placement.kind {
            Kind::Meter => node.set_role(Role::Meter),
            Kind::Indicator => node.set_role(Role::Status),
            Kind::Display if placement.element == Element::Message => {
                node.set_role(Role::Status);
            }
            _ => {}
        }
        if let Some(value) = look.value.as_ref().or(look.text.as_ref()) {
            node.set_value(value.clone());
        }
        if let Some(number) = look.number {
            node.set_numeric_value(number);
        }
        if live && placement.kind == Kind::Value {
            node.add_action(accesskit::Action::Increment);
            node.add_action(accesskit::Action::Decrement);
        }
        if let Some(description) = &look.description {
            node.set_description(description.clone());
        }
        if let Some(package) = placement.pending {
            node.set_description(format!("Not built yet: arrives with {package}."));
        }
        if !live {
            node.remove_action(accesskit::Action::Click);
            node.remove_action(accesskit::Action::Focus);
        }
    });
}

/// One option of a [`chooser`].
#[derive(Clone, Debug, PartialEq)]
pub struct Choice<T> {
    /// What it reads.
    pub label: String,
    /// What choosing it means.
    pub value: T,
}

/// A chooser (a drop-down) in the Settings window: a box showing the
/// current choice that opens a list when clicked. With the keyboard focus on
/// it, Up and Down step through the choices (S-SET-003). Returns the newly
/// chosen value.
#[expect(
    clippy::too_many_arguments,
    reason = "a chooser has this many parts, and every call site sets them all"
)]
pub fn chooser<T: Clone + PartialEq>(
    ui: &mut Ui,
    theme: &Theme,
    id_salt: &str,
    name: &str,
    rect: Rect,
    choices: &[Choice<T>],
    current: Option<&T>,
    enabled: bool,
) -> Option<T> {
    let id = Id::new(("chooser", id_salt));
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, id, sense);
    let index = current.and_then(|value| choices.iter().position(|c| c.value == *value));
    let text = index.map_or("", |i| choices[i].label.as_str()).to_owned();
    let mut chosen = None;

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
        let step = ui.input(|i| {
            i32::from(i.key_pressed(egui::Key::ArrowDown))
                - i32::from(i.key_pressed(egui::Key::ArrowUp))
        });
        if step != 0 && !choices.is_empty() {
            let from = index.map_or(if step > 0 { -1 } else { choices.len() as i32 }, |i| {
                i as i32
            });
            let next = (from + step).clamp(0, choices.len() as i32 - 1) as usize;
            if Some(next) != index {
                chosen = Some(choices[next].value.clone());
            }
        }
    }

    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(3.0));
    let face = if response.hovered() && enabled {
        c.control_hover.0
    } else {
        c.control.0
    };
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if enabled {
            face
        } else {
            face.gamma_multiply(0.55)
        },
    );
    let ink = if enabled { c.text.0 } else { c.pending.0 };
    // The text stops short of the arrow; a long choice is cut off there.
    let text_area = Rect::from_min_max(rect.min, pos2(rect.right() - 16.0, rect.bottom()));
    ui.painter_at(text_area).text(
        pos2(rect.left() + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        &text,
        theme.font(),
        ink,
    );
    let a = pos2(rect.right() - 9.0, rect.center().y);
    painter.add(Shape::convex_polygon(
        vec![
            a + vec2(-3.0, -1.5),
            a + vec2(3.0, -1.5),
            a + vec2(0.0, 2.0),
        ],
        ink,
        Stroke::NONE,
    ));
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Outside,
        );
    }

    response.widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, enabled, name));
    ui.ctx()
        .accesskit_node_builder(id, |node| node.set_value(text.clone()));

    egui::Popup::menu(&response).width(rect.width()).show(|ui| {
        for choice in choices {
            let selected = current == Some(&choice.value);
            if ui.selectable_label(selected, &choice.label).clicked() {
                chosen = Some(choice.value.clone());
            }
        }
    });
    chosen
}

/// A switch in the Settings window (54 points wide, L-SET-003): reads On or
/// Off. Returns the new state when it's clicked or Enter is pressed on it
/// (S-SET-004).
pub fn switch(
    ui: &mut Ui,
    theme: &Theme,
    id_salt: &str,
    name: &str,
    rect: Rect,
    on: bool,
) -> Option<bool> {
    let id = Id::new(("switch", id_salt));
    let response = ui.interact(rect, id, Sense::click());
    let pressed = response.clicked()
        || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(3.0));
    let face = if on { c.accent.0 } else { c.control.0 };
    painter.rect_filled(rect, theme.sizes.corner, face);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        if on { "On" } else { "Off" },
        theme.font(),
        if on { c.accent_text.0 } else { c.text.0 },
    );
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Outside,
        );
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, name));
    pressed.then_some(!on)
}

/// A number box in the Settings window, as Live's are: it shows the value
/// with its unit, and dragging up or down, Up and Down with the keyboard
/// focus on it, or a screen reader's increment and decrement step
/// it. `step` gives the value `n` steps from a value; the result is kept
/// within `range`. Returns the new value.
#[expect(
    clippy::too_many_arguments,
    reason = "a number box has this many parts, and every call site sets them all"
)]
pub fn number_box(
    ui: &mut Ui,
    theme: &Theme,
    id_salt: &str,
    name: &str,
    rect: Rect,
    value: f64,
    range: (f64, f64),
    step: &dyn Fn(f64, i32) -> f64,
    text: &str,
) -> Option<f64> {
    /// Points of drag per step.
    const DRAG_PER_STEP: f32 = 3.0;
    let id = Id::new(("number-box", id_salt));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let mut steps = 0i32;
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
        steps += ui.input(|i| {
            i32::from(i.key_pressed(egui::Key::ArrowUp))
                - i32::from(i.key_pressed(egui::Key::ArrowDown))
        });
    }
    steps += ui.input(|i| {
        i.num_accesskit_action_requests(id, accesskit::Action::Increment) as i32
            - i.num_accesskit_action_requests(id, accesskit::Action::Decrement) as i32
    });
    if response.dragged() {
        let whole = ui.data_mut(|d| {
            let pending = d.get_temp_mut_or(id, 0.0f32);
            *pending -= response.drag_delta().y;
            let whole = (*pending / DRAG_PER_STEP).trunc();
            *pending -= whole * DRAG_PER_STEP;
            whole
        });
        steps += whole as i32;
    } else if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<f32>(id));
    }
    let changed = (steps != 0)
        .then(|| step(value, steps).clamp(range.0, range.1))
        .filter(|next| (next - value).abs() > f64::EPSILON);

    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(3.0));
    let face = if response.hovered() || response.dragged() {
        c.control_hover.0
    } else {
        c.control.0
    };
    painter.rect_filled(rect, theme.sizes.corner, face);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.font(),
        c.text.0,
    );
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Outside,
        );
    }
    response.widget_info(|| WidgetInfo::slider(true, value, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::SpinButton);
        node.set_value(text.to_owned());
        node.set_numeric_value(value);
        node.set_min_numeric_value(range.0);
        node.set_max_numeric_value(range.1);
        node.add_action(accesskit::Action::Increment);
        node.add_action(accesskit::Action::Decrement);
    });
    changed
}
