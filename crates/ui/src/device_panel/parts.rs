//! The parts a device panel is made of (layout L-DEV-042 to L-DEV-046): the
//! two knobs, the boxes, the text buttons, the rows, stacks and grids of
//! buttons, the unit pair, the icons, the headings, the side strips and the
//! title bar's parts, at the sizes measured in Live 12.4.16 on 2 November
//! 2026. Each is drawn from the parameter's descriptor and the control's
//! place, and each change runs `device.set_param` as one undo step named for
//! the parameter, as every control of the panel does.

use std::f32::consts::{FRAC_PI_2, PI};

use bl_model::ParamId;
use bl_model::device_layout::{
    BOX_HEIGHT, Glyph, KNOB_DIAMETER, LABEL_HEIGHT, PlacedHeading, PlacedPages,
    SMALL_KNOB_DIAMETER, STRIP_HIDDEN, STRIP_SHOWN, STRIP_WIDTH, TitlePart, Unit, UnitPair,
};
use bl_shell::Invocation;
use bl_shell::device_commands::{DeviceFrame, DevicePanel};
use egui::accesskit::Role;
use egui::{
    Align2, Color32, FontId, Id, Key, Painter, Pos2, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use super::{Control, focus_ring, page_button_id, short_name};
use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{paint_ring_segment, ring_segment};

/// The size of the text on a device's controls, in points: Live's cap height
/// of 6.3 points (observed).
pub const DEVICE_TEXT: f32 = 9.0;

/// The corner of a box or a button, in points (observed: nearly square).
pub const BOX_CORNER: f32 = 1.0;

/// The stroke of a knob's ring and its lit arc, in points (observed).
const RING: f32 = 2.0;

/// The font of a device's labels, values and buttons.
#[must_use]
pub fn font() -> FontId {
    FontId::proportional(DEVICE_TEXT)
}

/// How a chooser's box looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ChooserStyle {
    /// A dark box.
    Plain,
    /// Filled like a lit number box.
    Lit,
    /// No box: the value in the accent colour and a small ▼.
    Bare,
}

/// What a control's face is made of while lit and while not, by the look of
/// the button.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    /// Dark while off.
    Dark,
    /// Outlined in the lit colour while off.
    Outlined,
}

impl Control<'_> {
    /// The name above the control, in a row of [`LABEL_HEIGHT`]; it stays as
    /// it is when the device is switched off (observed). Returns the rest.
    pub(super) fn label(&self, ui: &Ui, theme: &Theme) -> Rect {
        if self.unnamed {
            return self.rect;
        }
        let top = Rect::from_min_size(self.rect.min, vec2(self.rect.width(), LABEL_HEIGHT));
        ui.painter().text(
            top.center(),
            Align2::CENTER_CENTER,
            self.name(),
            font(),
            theme.colors.text.0,
        );
        Rect::from_min_max(pos2(self.rect.left(), top.bottom()), self.rect.max)
    }

    /// The name the control shows: its caption where the layout gives one.
    pub(super) fn name(&self) -> &str {
        self.caption
            .unwrap_or_else(|| short_name(&self.descriptor.name))
    }

    /// Whether the value is a lit (on) one of a switch.
    fn on(&self) -> bool {
        self.value >= 0.5
    }

    /// Whether the control was chosen: a click or Enter.
    fn chosen(ui: &Ui, response: &Response) -> bool {
        response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
    }

    /// A knob in one of Live's two sizes, its name above and its value below.
    /// The lit arc runs in the accent from the knob's low end, or from zero on
    /// a range that spans it, where a notch marks the place (observed on
    /// Pitch and Chord).
    pub(super) fn knob(&self, ui: &mut Ui, theme: &Theme, events: &mut Vec<UiEvent>, small: bool) {
        let below = self.label(ui, theme);
        let value_row =
            Rect::from_min_max(pos2(below.left(), below.bottom() - LABEL_HEIGHT), below.max);
        let face = Rect::from_min_max(below.min, pos2(below.right(), value_row.top()));
        let response = ui.interact(self.rect, self.id(), Sense::click_and_drag());
        crate::focus::register(ui.ctx(), self.id(), bl_shell::Area::Detail, None);
        self.value_input(ui, &response, events);
        let c = &theme.colors;
        let painter = ui.painter();
        let radius = if small {
            SMALL_KNOB_DIAMETER
        } else {
            KNOB_DIAMETER
        } / 2.0;
        // The ring hangs from the top of the face, its open foot unused.
        let centre = pos2(face.center().x, face.top() + radius);
        // The arc sweeps 300 degrees, from lower left to lower right.
        let (from, sweep) = (PI * 0.5 + PI / 6.0, PI * 5.0 / 3.0);
        let point = |to: f32| {
            let a = from + sweep * to;
            centre + vec2(a.cos(), a.sin()) * radius
        };
        let arc = |a: f32, b: f32| -> Vec<Pos2> {
            let steps = 32;
            (0..=steps)
                .map(|i| point(a + (b - a) * i as f32 / steps as f32))
                .collect()
        };
        painter.add(Shape::line(arc(0.0, 1.0), Stroke::new(RING, c.control.0)));
        let n = self.descriptor.normalize(self.shown()) as f32;
        let origin = self.origin();
        if (n - origin).abs() > 1e-4 {
            painter.add(Shape::line(
                arc(origin, n),
                Stroke::new(RING, self.lit(theme)),
            ));
        }
        if origin > 0.0 {
            // The notch over the zero a bipolar knob starts from, pointing
            // at the ring.
            let a = from + sweep * origin;
            let dir = vec2(a.cos(), a.sin());
            let side = vec2(-dir.y, dir.x) * 2.5;
            let base = centre + dir * (radius + 4.5);
            painter.add(Shape::convex_polygon(
                vec![base + side, base - side, centre + dir * (radius + 1.0)],
                c.control.0,
                Stroke::NONE,
            ));
        }
        // Spec: modulation R7: the pointer stays at the base, and the ring
        // shows the way to the value it plays.
        if let Some(segment) = self
            .modulated
            .and_then(|(_, played)| ring_segment(n, self.descriptor.normalize(played) as f32))
        {
            paint_ring_segment(painter, theme, segment, point);
        }
        painter.line_segment([centre, point(n)], Stroke::new(1.5, c.text.0));
        painter.text(
            value_row.center(),
            Align2::CENTER_CENTER,
            self.descriptor.format(self.shown()),
            font(),
            c.text.0,
        );
        focus_ring(ui, theme, self.rect, &response);
        self.describe(ui, &response, Role::Slider);
        self.knob_menus(&response, events);
    }

    /// Where along its sweep a knob's lit arc starts: the low end, or zero
    /// where the range spans it.
    fn origin(&self) -> f32 {
        let d = self.descriptor;
        if d.min() < 0.0 && d.max() > 0.0 {
            d.normalize(0.0) as f32
        } else {
            0.0
        }
    }

    /// A box: a dark field under the name, the value centred. A lit box
    /// shows its place in its range as a fill from the left (the value's
    /// fill colour); a marked box has a ◀ at its right.
    pub(super) fn number(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        events: &mut Vec<UiEvent>,
        (lit, marked): (bool, bool),
    ) {
        let below = self.label(ui, theme);
        let field = Rect::from_min_size(below.min, vec2(below.width(), BOX_HEIGHT));
        self.number_field(ui, theme, events, field, (lit, marked));
    }

    /// A box with its name at its left on the same line, `label_width`
    /// points of the control for the name: lit, with the name in the text
    /// colour, or plain with the name dim.
    pub(super) fn beside(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        events: &mut Vec<UiEvent>,
        (label_width, lit): (f32, bool),
    ) {
        ui.painter().text(
            pos2(self.rect.left(), self.rect.center().y),
            Align2::LEFT_CENTER,
            self.name(),
            font(),
            if lit {
                theme.colors.text.0
            } else {
                theme.colors.text_dim.0
            },
        );
        let field = Rect::from_min_max(
            pos2(self.rect.left() + label_width, self.rect.top()),
            self.rect.max,
        );
        self.number_field(ui, theme, events, field, (lit, false));
    }

    /// A button reading On or Off, lit while on, with the name at its left.
    pub(super) fn beside_switch(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        events: &mut Vec<UiEvent>,
        label_width: f32,
    ) {
        ui.painter().text(
            pos2(self.rect.left(), self.rect.center().y),
            Align2::LEFT_CENTER,
            self.name(),
            font(),
            theme.colors.text_dim.0,
        );
        let button = Rect::from_min_max(
            pos2(self.rect.left() + label_width, self.rect.top()),
            self.rect.max,
        );
        let on = self.on();
        let text = self.descriptor.format(self.value);
        let response = self.button(
            ui,
            theme,
            (button, self.id()),
            (Look::Dark, on),
            Face::Text(&text),
        );
        if Self::chosen(ui, &response) {
            self.change(if on { 0.0 } else { 1.0 }, events);
        }
        self.describe_switch(ui, &response);
    }

    /// A small square, filled while on, with the name after it.
    pub(super) fn check(&self, ui: &mut Ui, theme: &Theme, events: &mut Vec<UiEvent>) {
        let response = ui.interact(self.rect, self.id(), Sense::click());
        crate::focus::register(ui.ctx(), self.id(), bl_shell::Area::Detail, None);
        let on = self.on();
        let c = &theme.colors;
        let painter = ui.painter();
        let square = Rect::from_center_size(
            pos2(self.rect.left() + 4.5, self.rect.center().y),
            vec2(8.0, 8.0),
        );
        painter.rect_stroke(
            square,
            BOX_CORNER,
            Stroke::new(1.0, self.lit(theme)),
            StrokeKind::Inside,
        );
        if on {
            painter.rect_filled(square.shrink(2.0), 0.0, self.lit(theme));
        }
        painter.text(
            pos2(square.right() + 4.0, self.rect.center().y),
            Align2::LEFT_CENTER,
            self.name(),
            font(),
            c.text_dim.0,
        );
        if Self::chosen(ui, &response) {
            self.change(if on { 0.0 } else { 1.0 }, events);
        }
        focus_ring(ui, theme, self.rect, &response);
        self.describe_switch(ui, &response);
    }

    /// The dim name over the value, both from the control's left: the name
    /// of a bare control, in the row above the value's.
    fn dim_label(&self, ui: &Ui, theme: &Theme) -> Rect {
        if self.unnamed {
            return self.rect;
        }
        let top = Rect::from_min_size(self.rect.min, vec2(self.rect.width(), LABEL_HEIGHT));
        ui.painter().text(
            top.left_center(),
            Align2::LEFT_CENTER,
            self.name(),
            font(),
            theme.colors.text_dim.0,
        );
        Rect::from_min_max(pos2(self.rect.left(), top.bottom()), self.rect.max)
    }

    /// A number with no box: the value in the accent from the left, under its
    /// dim name.
    pub(super) fn bare(&self, ui: &mut Ui, theme: &Theme, events: &mut Vec<UiEvent>) {
        let below = self.dim_label(ui, theme);
        let field = Rect::from_min_size(below.min, vec2(below.width(), BOX_HEIGHT));
        let response = ui.interact(field, self.id(), Sense::click_and_drag());
        crate::focus::register(ui.ctx(), self.id(), bl_shell::Area::Detail, None);
        self.value_input(ui, &response, events);
        ui.painter().text(
            field.left_center(),
            Align2::LEFT_CENTER,
            self.text
                .clone()
                .unwrap_or_else(|| self.descriptor.format(self.value)),
            font(),
            self.lit(theme),
        );
        focus_ring(ui, theme, field, &response);
        self.describe(ui, &response, Role::SpinButton);
    }

    /// A name in the dim colour with the value after it in the accent: no box.
    pub(super) fn readout(&self, ui: &mut Ui, theme: &Theme, events: &mut Vec<UiEvent>) {
        let c = &theme.colors;
        let painter = ui.painter();
        let name = self.name();
        let value = self.descriptor.format(self.value);
        let mid = pos2(self.rect.left(), self.rect.center().y);
        let used = painter
            .text(mid, Align2::LEFT_CENTER, name, font(), c.text_dim.0)
            .width();
        let field = Rect::from_min_max(
            pos2(self.rect.left() + used + 4.0, self.rect.top()),
            self.rect.max,
        );
        let response = ui.interact(field, self.id(), Sense::click_and_drag());
        crate::focus::register(ui.ctx(), self.id(), bl_shell::Area::Detail, None);
        self.value_input(ui, &response, events);
        ui.painter().text(
            pos2(field.left(), field.center().y),
            Align2::LEFT_CENTER,
            value,
            font(),
            self.lit(theme),
        );
        focus_ring(ui, theme, field, &response);
        self.describe(ui, &response, Role::SpinButton);
    }

    /// A pill with a dot: where the dot sits shows which choice is chosen,
    /// and a click moves it to the next, round to the first.
    pub(super) fn pill(&self, ui: &mut Ui, theme: &Theme, events: &mut Vec<UiEvent>) {
        let response = ui.interact(self.rect, self.id(), Sense::click());
        crate::focus::register(ui.ctx(), self.id(), bl_shell::Area::Detail, None);
        let c = &theme.colors;
        let painter = ui.painter();
        painter.rect_filled(self.rect, self.rect.height() / 2.0, c.window.0);
        let steps = match &self.descriptor.kind {
            bl_model::ParamKind::Choice { choices } => choices.len().max(2),
            _ => 2,
        };
        let radius = self.rect.height() / 2.0 - 2.0;
        let span = self.rect.width() - 2.0 * (radius + 2.0);
        let at = self.value.round().clamp(0.0, (steps - 1) as f64) as f32 / (steps - 1) as f32;
        painter.circle_filled(
            pos2(
                self.rect.left() + radius + 2.0 + span * at,
                self.rect.center().y,
            ),
            radius,
            c.text_dim.0,
        );
        if Self::chosen(ui, &response) {
            self.change(((self.value.round() as usize + 1) % steps) as f64, events);
        }
        focus_ring(ui, theme, self.rect, &response);
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Button);
            node.set_label(self.descriptor.name.clone());
            node.set_value(self.descriptor.format(self.value));
        });
    }

    /// The box of a number control at `field`.
    fn number_field(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        events: &mut Vec<UiEvent>,
        field: Rect,
        (lit, marked): (bool, bool),
    ) {
        let response = ui.interact(field, self.id(), Sense::click_and_drag());
        crate::focus::register(ui.ctx(), self.id(), bl_shell::Area::Detail, None);
        self.value_input(ui, &response, events);
        let c = &theme.colors;
        let painter = ui.painter();
        painter.rect_filled(field, BOX_CORNER, c.window.0);
        if lit {
            let n = self.descriptor.normalize(self.value) as f32;
            let fill = if self.enabled {
                c.value_fill.0
            } else {
                super::highlight(theme, false)
            };
            painter.rect_filled(
                Rect::from_min_size(field.min, vec2(field.width() * n, field.height())),
                BOX_CORNER,
                fill,
            );
        }
        painter.text(
            field.center(),
            Align2::CENTER_CENTER,
            self.text
                .clone()
                .unwrap_or_else(|| self.descriptor.format(self.value)),
            font(),
            c.text.0,
        );
        if marked {
            paint_glyph(
                painter,
                Rect::from_center_size(pos2(field.right() - 6.0, field.center().y), vec2(6.0, 6.0)),
                Glyph::Left,
                c.text_dim.0,
            );
        }
        focus_ring(ui, theme, field, &response);
        self.describe(ui, &response, Role::SpinButton);
    }

    /// A chooser: a dark box under the name with the value at its left and a
    /// ▼ at its right, opening a menu of the choices.
    pub(super) fn chooser(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        events: &mut Vec<UiEvent>,
        style: ChooserStyle,
    ) {
        let below = if style == ChooserStyle::Bare {
            self.dim_label(ui, theme)
        } else {
            self.label(ui, theme)
        };
        let field = Rect::from_min_size(below.min, vec2(below.width(), BOX_HEIGHT));
        let response = ui.interact(field, self.id(), Sense::click());
        crate::focus::register(ui.ctx(), self.id(), bl_shell::Area::Detail, None);
        let c = &theme.colors;
        let painter = ui.painter();
        let (fill, ink) = match style {
            ChooserStyle::Plain => (Some(c.window.0), c.text.0),
            ChooserStyle::Lit => (Some(c.value_fill.0), c.text.0),
            ChooserStyle::Bare => (None, c.value_fill.0),
        };
        if let Some(fill) = fill {
            painter.rect_filled(field, BOX_CORNER, fill);
        }
        let text = self.descriptor.format(self.value);
        if style == ChooserStyle::Bare {
            // The value from the box's left, the ▼ after it.
            let used = painter
                .text(
                    pos2(field.left(), field.center().y),
                    Align2::LEFT_CENTER,
                    &text,
                    font(),
                    ink,
                )
                .width();
            paint_glyph(
                painter,
                Rect::from_center_size(
                    pos2(field.left() + used + 8.0, field.center().y),
                    vec2(4.0, 4.0),
                ),
                Glyph::Down,
                c.text.0,
            );
        } else {
            painter.text(
                pos2(field.left() + 3.0, field.center().y),
                Align2::LEFT_CENTER,
                text,
                font(),
                ink,
            );
            paint_glyph(
                painter,
                Rect::from_center_size(pos2(field.right() - 6.0, field.center().y), vec2(5.0, 5.0)),
                Glyph::Down,
                c.text.0,
            );
        }
        focus_ring(ui, theme, field, &response);
        if let bl_model::ParamKind::Choice { choices } = &self.descriptor.kind {
            egui::Popup::menu(&response).show(|ui| {
                for (index, choice) in choices.iter().enumerate() {
                    if ui
                        .selectable_label(index as f64 == self.value, choice)
                        .clicked()
                    {
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

    /// A button over the whole control: lit while on, with `face` drawn on it
    /// (the parameter's name, or an icon). `look` says how it reads while
    /// off. Returns whether it was chosen.
    fn button(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        (rect, id): (Rect, Id),
        (look, lit): (Look, bool),
        face: Face<'_>,
    ) -> Response {
        let response = ui.interact(rect, id, Sense::click());
        crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
        let c = &theme.colors;
        let accent = self.lit(theme);
        let (fill, ink) = match (lit, look) {
            (true, _) => (accent, c.accent_text.0),
            (false, Look::Dark) => (
                if response.hovered() {
                    c.control_hover.0
                } else {
                    c.window.0
                },
                c.text.0,
            ),
            (false, Look::Outlined) => (Color32::TRANSPARENT, accent),
        };
        let painter = ui.painter();
        match face {
            Face::Circle(glyph) => {
                painter.circle_filled(rect.center(), rect.height() / 2.0, fill);
                if let Some(glyph) = glyph {
                    paint_glyph(painter, rect.shrink(3.0), glyph, ink);
                }
            }
            face => {
                painter.rect_filled(rect, BOX_CORNER, fill);
                if look == Look::Outlined && !lit {
                    painter.rect_stroke(
                        rect.shrink(0.5),
                        BOX_CORNER,
                        Stroke::new(1.0, accent),
                        StrokeKind::Inside,
                    );
                }
                match face {
                    Face::Text(text) => {
                        painter.text(rect.center(), Align2::CENTER_CENTER, text, font(), ink);
                    }
                    Face::Glyph(glyph) => paint_glyph(painter, rect.shrink(2.0), glyph, ink),
                    Face::Circle(_) => {}
                }
            }
        }
        focus_ring(ui, theme, rect, &response);
        response
    }

    /// A text button holding the parameter's name: lit while on.
    pub(super) fn toggle(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        events: &mut Vec<UiEvent>,
        outlined: bool,
    ) {
        let look = if outlined { Look::Outlined } else { Look::Dark };
        let on = self.on();
        let name = self.name();
        let response = self.button(
            ui,
            theme,
            (self.rect, self.id()),
            (look, on),
            Face::Text(name),
        );
        if Self::chosen(ui, &response) {
            self.change(if on { 0.0 } else { 1.0 }, events);
        }
        self.describe_switch(ui, &response);
    }

    /// A single button showing the chosen choice, lit: a click takes the next.
    pub(super) fn cycle(&self, ui: &mut Ui, theme: &Theme, events: &mut Vec<UiEvent>) {
        let text = self.descriptor.format(self.value);
        let response = self.button(
            ui,
            theme,
            (self.rect, self.id()),
            (
                Look::Dark,
                !matches!(self.descriptor.kind, bl_model::ParamKind::Toggle) || self.on(),
            ),
            Face::Text(&text),
        );
        if Self::chosen(ui, &response) {
            let count = match &self.descriptor.kind {
                bl_model::ParamKind::Choice { choices } => choices.len().max(1),
                bl_model::ParamKind::Toggle => 2,
                _ => 1,
            };
            self.change(((self.value.round() as usize + 1) % count) as f64, events);
        }
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Button);
            node.set_label(self.descriptor.name.clone());
            node.set_value(text.clone());
        });
    }

    /// An icon button or a round one: lit while on.
    pub(super) fn icon_toggle(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        events: &mut Vec<UiEvent>,
        (glyph, round): (Option<Glyph>, bool),
    ) {
        let on = self.on();
        let face = if round {
            Face::Circle(glyph)
        } else if let Some(glyph) = glyph {
            Face::Glyph(glyph)
        } else {
            Face::Text(self.name())
        };
        let response = self.button(ui, theme, (self.rect, self.id()), (Look::Dark, on), face);
        if Self::chosen(ui, &response) {
            self.change(if on { 0.0 } else { 1.0 }, events);
        }
        self.describe_switch(ui, &response);
    }

    fn describe_switch(&self, ui: &Ui, response: &Response) {
        let on = self.on();
        let name = self.descriptor.name.clone();
        response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, &name));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Switch);
            node.set_label(self.descriptor.name.clone());
            node.set_value(self.descriptor.format(self.value));
        });
    }

    /// The choices of a choice parameter as buttons in `cells`, the chosen
    /// one lit: a row, a stack or a grid. `icons` draws each as the icon of
    /// its name.
    pub(super) fn choice_buttons(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        events: &mut Vec<UiEvent>,
        (cells, icons): (&[Rect], bool),
    ) {
        let bl_model::ParamKind::Choice { choices } = &self.descriptor.kind else {
            return;
        };
        for (index, (rect, choice)) in cells.iter().zip(choices).enumerate() {
            let chosen = index as f64 == self.value;
            let glyph = icons.then(|| glyph_for(choice)).flatten();
            let id = self.id().with(index);
            let face = glyph.map_or(Face::Text(choice), Face::Glyph);
            let response = self.button(ui, theme, (*rect, id), (Look::Dark, chosen), face);
            if Self::chosen(ui, &response) && !chosen {
                self.change(index as f64, events);
            }
            let (name, choice) = (self.descriptor.name.clone(), choice.clone());
            response.widget_info(|| {
                WidgetInfo::selected(
                    WidgetType::RadioButton,
                    true,
                    chosen,
                    format!("{name} {choice}"),
                )
            });
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::RadioButton);
                node.set_label(format!("{} {choice}", self.descriptor.name));
                node.set_selected(chosen);
            });
        }
    }

    /// The unit pair beside a knob: two stacked buttons, the unit the knob
    /// reads in lit. A press sets the switch that turns the knob between its
    /// two parameters.
    pub(super) fn unit_pair(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        events: &mut Vec<UiEvent>,
        pair: &UnitPair,
        mode: &Control<'_>,
    ) {
        let top = mode.rect.center().y - BOX_HEIGHT - 1.5;
        let on_second = mode.on();
        for (index, unit) in pair.units.iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(mode.rect.left(), top + index as f32 * (BOX_HEIGHT + 3.0)),
                vec2(bl_model::device_layout::UNIT_PAIR_WIDTH, BOX_HEIGHT),
            );
            let chosen = on_second == (index == 1);
            let face = match (unit.glyph(), unit.text()) {
                (Some(glyph), _) => Face::Glyph(glyph),
                (None, Some(text)) => Face::Text(text),
                (None, None) => Face::Text(""),
            };
            let response = self.button(
                ui,
                theme,
                (rect, mode.id().with(index)),
                (Look::Dark, chosen),
                face,
            );
            if Self::chosen(ui, &response) && !chosen {
                mode.change(index as f64, events);
            }
            let label = unit_name(*unit);
            response
                .widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, chosen, label));
            ui.ctx()
                .accesskit_node_builder(mode.id().with(index), |node| {
                    node.set_role(Role::RadioButton);
                    node.set_label(format!("{}: {}", mode.descriptor.name, unit_name(*unit)));
                    node.set_selected(chosen);
                });
        }
    }
}

/// What a button's face shows.
enum Face<'a> {
    /// Text.
    Text(&'a str),
    /// An icon on a small rectangular button.
    Glyph(Glyph),
    /// A round button, with an icon if it has one.
    Circle(Option<Glyph>),
}

/// How a unit reads to a screen reader.
fn unit_name(unit: Unit) -> &'static str {
    match unit {
        Unit::Ms => "milliseconds",
        Unit::Hz => "hertz",
        Unit::Sync => "synced to the tempo",
        Unit::Note => "note",
    }
}

/// The icon of a choice named for a filter type, as the filter-type buttons
/// show them; `None` for any other name, which draws as text.
#[must_use]
pub fn glyph_for(choice: &str) -> Option<Glyph> {
    let name = choice.to_ascii_lowercase();
    if name.contains("low") || name == "lp" {
        Some(Glyph::LowPass)
    } else if name.contains("high") || name == "hp" {
        Some(Glyph::HighPass)
    } else if name.contains("band") || name == "bp" {
        Some(Glyph::BandPass)
    } else if name.contains("notch") || name == "n" {
        Some(Glyph::Notch)
    } else if name == "sync" {
        Some(Glyph::Metronome)
    } else if name == "crossfade" {
        Some(Glyph::Crossfade)
    } else if name == "envelope" {
        Some(Glyph::Envelope)
    } else if name == "phase" {
        Some(Glyph::Phase)
    } else if name == "spin" {
        Some(Glyph::Spin)
    } else {
        None
    }
}

/// Draws `glyph` in `rect` in `ink`.
pub fn paint_glyph(painter: &Painter, rect: Rect, glyph: Glyph, ink: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) / 2.0;
    let at = |x: f32, y: f32| pos2(c.x + x * s, c.y + y * s);
    let stroke = Stroke::new(1.0, ink);
    let line = |points: &[(f32, f32)]| {
        painter.add(Shape::line(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            stroke,
        ));
    };
    let fill = |points: &[(f32, f32)]| {
        painter.add(Shape::convex_polygon(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            ink,
            Stroke::NONE,
        ));
    };
    match glyph {
        Glyph::Crossfade => {
            line(&[(-0.9, -0.6), (0.9, 0.6)]);
            line(&[(-0.9, 0.6), (0.9, -0.6)]);
        }
        Glyph::Envelope => line(&[(-0.9, 0.6), (0.0, -0.6), (0.9, 0.6)]),
        Glyph::Phase => {
            painter.circle_stroke(c, s * 0.7, stroke);
            line(&[(-0.9, 0.9), (0.9, -0.9)]);
        }
        Glyph::Spin => {
            painter.circle_stroke(c, s * 0.7, stroke);
            fill(&[(0.7, -0.5), (1.0, 0.0), (0.4, 0.0)]);
        }
        Glyph::LowPass => line(&[(-0.9, -0.3), (0.0, -0.3), (0.5, -0.1), (0.9, 0.6)]),
        Glyph::HighPass => line(&[(0.9, -0.3), (0.0, -0.3), (-0.5, -0.1), (-0.9, 0.6)]),
        Glyph::BandPass => line(&[
            (-0.9, 0.5),
            (-0.4, 0.4),
            (-0.2, -0.4),
            (0.2, -0.4),
            (0.4, 0.4),
            (0.9, 0.5),
        ]),
        Glyph::Notch => line(&[
            (-0.9, -0.3),
            (-0.2, -0.3),
            (0.0, 0.5),
            (0.2, -0.3),
            (0.9, -0.3),
        ]),
        Glyph::Metronome => {
            line(&[
                (-0.6, 0.7),
                (-0.3, -0.7),
                (0.3, -0.7),
                (0.6, 0.7),
                (-0.6, 0.7),
            ]);
            line(&[(0.0, 0.4), (0.35, -0.5)]);
        }
        Glyph::Play => fill(&[(-0.5, -0.7), (0.7, 0.0), (-0.5, 0.7)]),
        Glyph::Down => fill(&[(-0.7, -0.4), (0.7, -0.4), (0.0, 0.6)]),
        Glyph::Left => fill(&[(0.5, -0.7), (-0.6, 0.0), (0.5, 0.7)]),
        Glyph::Right => fill(&[(-0.5, -0.7), (0.6, 0.0), (-0.5, 0.7)]),
        Glyph::Info => {
            painter.circle_filled(at(0.0, -0.65), s * 0.18, ink);
            line(&[(0.0, -0.2), (0.0, 0.7)]);
        }
        Glyph::Plus => {
            line(&[(-0.6, 0.0), (0.6, 0.0)]);
            line(&[(0.0, -0.6), (0.0, 0.6)]);
        }
        Glyph::Minus => line(&[(-0.6, 0.0), (0.6, 0.0)]),
        Glyph::FlatSharp => {
            // A flat, then a sharp.
            line(&[(-0.8, -0.8), (-0.8, 0.6)]);
            line(&[(-0.8, 0.6), (-0.3, 0.2), (-0.3, -0.1), (-0.8, 0.1)]);
            line(&[(0.35, -0.8), (0.35, 0.8)]);
            line(&[(0.75, -0.8), (0.75, 0.8)]);
            line(&[(0.1, -0.15), (1.0, -0.4)]);
            line(&[(0.1, 0.4), (1.0, 0.15)]);
        }
    }
}

/// A section heading over a group of controls.
pub(super) fn heading(ui: &Ui, theme: &Theme, placed: &PlacedHeading, origin: Pos2) {
    let rect = Rect::from_min_size(
        origin + vec2(placed.rect.x, placed.rect.y),
        vec2(placed.rect.width, placed.rect.height),
    );
    let (at, align) = if placed.centered {
        (rect.center(), Align2::CENTER_CENTER)
    } else {
        (rect.left_center() + vec2(2.0, 0.0), Align2::LEFT_CENTER)
    };
    ui.painter()
        .text(at, align, &placed.text, font(), theme.colors.text.0);
}

/// A side strip: its ▼ toggle at the top, its name up its length, a line down
/// its right edge. The toggle shows and hides the section beside it.
pub(super) fn side_strip(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    (placed, toggle, origin): (&PlacedPages, Rect, Pos2),
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let area = Rect::from_min_size(
        origin + vec2(placed.rect.x, placed.rect.y),
        vec2(STRIP_WIDTH, placed.rect.height),
    );
    let shown = placed
        .names
        .get(placed.shown)
        .is_some_and(|n| n == STRIP_SHOWN);
    let painter = ui.painter_at(area);
    painter.vline(
        area.right() - 0.5,
        area.y_range(),
        Stroke::new(1.0, c.separator.0),
    );
    let galley = painter.layout_no_wrap(placed.name.clone(), font(), c.text_dim.0);
    // Written upwards from the strip's foot.
    let at = pos2(area.center().x - galley.size().y / 2.0, area.bottom() - 4.0);
    painter.add(egui::epaint::TextShape::new(at, galley, c.text_dim.0).with_angle(-FRAC_PI_2));
    let id = page_button_id(frame.id, &placed.name, "strip");
    let response = ui.interact(toggle, id, Sense::click());
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    let (fill, ink) = if shown {
        (c.accent.0, c.accent_text.0)
    } else {
        (c.window.0, c.text.0)
    };
    painter.circle_filled(toggle.center(), toggle.width() / 2.0, fill);
    paint_glyph(&painter, toggle.shrink(3.0), Glyph::Down, ink);
    focus_ring(ui, theme, toggle, &response);
    if Control::chosen(ui, &response) {
        let page = if shown { STRIP_HIDDEN } else { STRIP_SHOWN };
        events.push(UiEvent::Invoke(Invocation::new(
            bl_shell::device_commands::SET_PAGE,
            json!({ "device": frame.id, "node": placed.name, "page": page }),
        )));
    }
    let name = placed.name.clone();
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, shown, &name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Switch);
        node.set_label(placed.name.clone());
        node.set_value(if shown { "shown" } else { "hidden" }.to_owned());
    });
}

/// The width of a title-bar text in `font`.
fn text_width(painter: &Painter, text: &str, ink: Color32) -> f32 {
    painter
        .layout_no_wrap(text.to_owned(), font(), ink)
        .size()
        .x
}

/// The longest start of `name` that `fits`, ending in `…` where it was cut:
/// a device whose name is wider than its title bar shows ("Note Le…").
#[must_use]
pub fn truncated(name: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(name) {
        return name.to_owned();
    }
    let chars: Vec<char> = name.chars().collect();
    (1..chars.len())
        .rev()
        .map(|n| format!("{}…", chars[..n].iter().collect::<String>()))
        .find(|candidate| fits(candidate))
        .unwrap_or_else(|| "…".to_owned())
}

/// Draws the parts a device adds to its title bar (L-DEV-045): round buttons
/// after the activator, and from the right end the others in turn, the last
/// of the list outermost. Returns the room that is left for the name and the
/// tabs.
pub fn title_parts(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    title: Rect,
    events: &mut Vec<UiEvent>,
) -> Rect {
    let Some(panel) = frame.panel.as_ref() else {
        return title;
    };
    let c = &theme.colors;
    let mut room = title;
    let painter = ui.painter_at(title.expand(2.0));
    let row = |x: f32, width: f32| {
        Rect::from_min_size(
            pos2(x, title.center().y - BOX_HEIGHT / 2.0),
            vec2(width, BOX_HEIGHT),
        )
    };
    for (index, part) in panel.description.layout.title.iter().enumerate() {
        if let TitlePart::Round(glyph, param) = part {
            let rect = row(room.left(), BOX_HEIGHT);
            room.min.x = rect.right() + 3.0;
            title_button(
                ui,
                theme,
                (frame, panel),
                (index, *param),
                rect,
                Face::Circle(Some(*glyph)),
                events,
            );
        }
    }
    for (index, part) in panel.description.layout.title.iter().enumerate().rev() {
        let ink = c.text_dim.0;
        match part {
            TitlePart::Round(..) => {}
            TitlePart::Heading(text) => {
                let width = text_width(&painter, text, ink);
                let rect = row(room.right() - width, width);
                room.max.x = rect.left() - 6.0;
                painter.text(
                    rect.left_center(),
                    Align2::LEFT_CENTER,
                    text,
                    font(),
                    c.text.0,
                );
            }
            TitlePart::Mpe => {
                let width = text_width(&painter, "MPE", ink) + 6.0;
                let rect = row(room.right() - width, width);
                room.max.x = rect.left() - 3.0;
                painter.rect_stroke(
                    rect.shrink(0.5),
                    BOX_CORNER,
                    Stroke::new(1.0, ink),
                    StrokeKind::Inside,
                );
                painter.text(rect.center(), Align2::CENTER_CENTER, "MPE", font(), ink);
            }
            TitlePart::Maker(maker) => {
                let text = maker.text();
                let width = text_width(&painter, text, ink);
                let rect = row(room.right() - width, width);
                room.max.x = rect.left() - 6.0;
                painter.text(rect.left_center(), Align2::LEFT_CENTER, text, font(), ink);
            }
            TitlePart::Letter(letter, param) => {
                let rect = row(room.right() - BOX_HEIGHT, BOX_HEIGHT);
                room.max.x = rect.left() - 3.0;
                let text = letter.to_string();
                title_button(
                    ui,
                    theme,
                    (frame, panel),
                    (index, *param),
                    rect,
                    Face::Text(&text),
                    events,
                );
            }
            TitlePart::Button(param) => {
                let name = panel
                    .description
                    .params
                    .iter()
                    .find(|p| p.id == *param)
                    .map_or("", |p| p.name.as_str());
                let width = text_width(&painter, name, ink) + 10.0;
                let rect = row(room.right() - width, width);
                room.max.x = rect.left() - 3.0;
                title_button(
                    ui,
                    theme,
                    (frame, panel),
                    (index, *param),
                    rect,
                    Face::Text(name),
                    events,
                );
            }
        }
    }
    room
}

/// A button of the title bar for `param`: lit while it is on.
fn title_button(
    ui: &mut Ui,
    theme: &Theme,
    (frame, panel): (&DeviceFrame, &DevicePanel),
    (index, param): (usize, ParamId),
    rect: Rect,
    face: Face<'_>,
    events: &mut Vec<UiEvent>,
) {
    let Some(at) = panel.description.params.iter().position(|p| p.id == param) else {
        return;
    };
    let control = plain_control(frame, panel, at, rect);
    let id = Id::new(("device-title-part", frame.id, index));
    let on = control.on();
    let response = control.button(ui, theme, (rect, id), (Look::Dark, on), face);
    if Control::chosen(ui, &response) {
        control.change(if on { 0.0 } else { 1.0 }, events);
    }
    control.describe_switch(ui, &response);
}

/// A control of the parameter at `at` in the panel, drawn at `rect`: for the
/// buttons that set a parameter the layout draws beside another control.
pub(super) fn plain_control<'a>(
    frame: &DeviceFrame,
    panel: &'a DevicePanel,
    at: usize,
    rect: Rect,
) -> Control<'a> {
    Control {
        device: frame.id,
        enabled: frame.enabled,
        descriptor: &panel.description.params[at],
        value: panel.value(at),
        modulated: None,
        routes: &[],
        rect,
        stops: &[],
        knob: None,
        configured: &[],
        text: None,
        automation: None,
        coarse: 1,
        unnamed: false,
        caption: None,
    }
}
