//! The generators' controls in the Generate panel (WP-6.14; the MIDI
//! generators spec, R2 to R16): Rhythm's step pattern and Accent Offset
//! arrows, Seed's pitch-range slider, Shape's Shape Levels display, and
//! Euclidean's Pattern and Voices tabs. A tool's other controls are the
//! panel's own, drawn from its description; every control here runs
//! `midi_tool.set` (or `midi_tool.randomize`), and a drag is a gesture of it:
//! one undo step named after the tool, and Esc puts the notes back.

use bl_midi_tools::{
    EUCLIDEAN_VOICES, MAX_CHORDS, ParamKind, ParamSpec, ParamValue, StackView, euclid, key_name,
    rhythm_steps, rotation_id, stack_view,
};
use bl_model::ClipId;
use bl_shell::midi_tools::ToolView;
use bl_shell::{Area, GesturePhase, Invocation, Shell};
use egui::accesskit::{self, Role};
use egui::{
    Align2, Id, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::tool_panel::{
    CONTROL, CONTROLS_AT, ROW, Shown, button, control, control_info, end_gesture, set,
};

/// A step cell's greatest width, and the height of a strip of them.
const CELL: f32 = 9.0;
const STRIP: f32 = 9.0;
/// How wide a voice's switch is, and the controls beside it.
const SWITCH: f32 = 36.0;
const SMALL: f32 = 44.0;
/// How tall the Shape Levels display is.
const LEVELS_HEIGHT: f32 = 40.0;
/// How far an arrow key moves a level.
const LEVEL_STEP: f64 = 0.05;
/// An arrow button's width.
const ARROW: f32 = 18.0;
/// The Chord Selector Pad's diagrams: how many show at once, across and
/// down, and how big each is.
const PAD_COLUMNS: usize = 3;
const PAD_ROWS: usize = 3;
const DIAGRAM: egui::Vec2 = vec2(30.0, 24.0);
const DIAGRAM_GAP: f32 = 2.0;

/// Draws the rows of Rhythm, Seed, Shape, Stacks or Euclidean from `y` down and
/// returns where the next goes; `None` for any other tool, whose rows the
/// panel draws.
#[expect(clippy::too_many_arguments, reason = "a panel's whole context")]
pub(crate) fn rows(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    y: f32,
    clip: ClipId,
    view: &ToolView<'_>,
    events: &mut Vec<UiEvent>,
) -> Option<f32> {
    let panel = Panel {
        shell,
        theme,
        rect,
        clip,
        view,
    };
    Some(match view.descriptor.id.as_str() {
        "rhythm" => panel.rhythm(ui, y, events),
        "seed" => panel.seed(ui, y, events),
        "shape" => panel.shape(ui, y, events),
        "euclidean" => panel.euclidean(ui, y, events),
        "stacks" => panel.stacks(ui, y, events),
        _ => return None,
    })
}

/// What a generator's rows are drawn from.
struct Panel<'a> {
    shell: &'a Shell,
    theme: &'a Theme,
    rect: Rect,
    clip: ClipId,
    view: &'a ToolView<'a>,
}

impl Panel<'_> {
    fn left(&self) -> f32 {
        self.rect.left() + 6.0
    }

    fn width(&self) -> f32 {
        self.rect.width() - 12.0
    }

    fn spec(&self, id: &str) -> (usize, &ParamSpec) {
        self.view
            .descriptor
            .parameter(id)
            .unwrap_or_else(|| panic!("{} has no parameter `{id}`", self.view.descriptor.name))
    }

    fn value(&self, id: &str) -> &ParamValue {
        &self.view.values[self.spec(id).0]
    }

    fn whole(&self, id: &str) -> i64 {
        match self.value(id) {
            ParamValue::Whole(n) => *n,
            _ => 0,
        }
    }

    fn name_at(&self, ui: &Ui, y: f32, text: &str) {
        ui.painter().with_clip_rect(self.rect).text(
            pos2(self.left(), y + CONTROL.y / 2.0),
            Align2::LEFT_CENTER,
            text,
            self.theme.small_font(),
            self.theme.colors.text_dim.0,
        );
    }

    /// The control of parameter `id` in `at`.
    fn control(&self, ui: &mut Ui, id: &str, at: Rect, events: &mut Vec<UiEvent>) {
        let (index, spec) = self.spec(id);
        control(
            ui,
            self.theme,
            self.clip,
            self.view.descriptor,
            spec,
            &self.view.values[index],
            Shown {
                at,
                text: None,
                enabled: true,
            },
            events,
        );
    }

    /// A row: the parameter's name and its control. Returns where the next
    /// row goes.
    fn row(&self, ui: &mut Ui, y: f32, id: &str, events: &mut Vec<UiEvent>) -> f32 {
        self.name_at(ui, y, &self.spec(id).1.name);
        let at = Rect::from_min_size(pos2(self.rect.left() + CONTROLS_AT, y), CONTROL);
        self.control(ui, id, at, events);
        y + ROW
    }

    /// A row of a range's two ends, each a control of its own parameter.
    fn pair(
        &self,
        ui: &mut Ui,
        y: f32,
        name: &str,
        (low, high): (&str, &str),
        events: &mut Vec<UiEvent>,
    ) -> f32 {
        self.name_at(ui, y, name);
        let half = (CONTROL.x - 4.0) / 2.0;
        for (index, id) in [low, high].into_iter().enumerate() {
            let at = Rect::from_min_size(
                pos2(
                    self.rect.left() + CONTROLS_AT + (half + 4.0) * index as f32,
                    y,
                ),
                vec2(half, CONTROL.y),
            );
            self.control(ui, id, at, events);
        }
        y + ROW
    }

    /// A strip of step cells, the ones in `hits` lit, `dim` when the voice is
    /// switched off.
    fn strip(&self, ui: &mut Ui, y: f32, hits: &[bool], dim: bool, name: &str) -> f32 {
        let gap = 1.0;
        let each = ((self.width() + gap) / hits.len().max(1) as f32 - gap).min(CELL);
        let strip = Rect::from_min_size(
            pos2(self.left(), y),
            vec2((each + gap) * hits.len() as f32 - gap, STRIP),
        );
        let c = &self.theme.colors;
        for (at, hit) in hits.iter().enumerate() {
            let cell = Rect::from_min_size(
                pos2(self.left() + (each + gap) * at as f32, y),
                vec2(each, STRIP),
            );
            let colour = match (hit, dim) {
                (true, false) => c.accent.0,
                (true, true) => c.accent.0.gamma_multiply(0.4),
                (false, _) => c.control.0.gamma_multiply(if dim { 0.55 } else { 1.0 }),
            };
            ui.painter()
                .rect_filled(cell, self.theme.sizes.corner, colour);
        }
        let steps: Vec<String> = hits
            .iter()
            .enumerate()
            .filter(|(_, hit)| **hit)
            .map(|(at, _)| (at + 1).to_string())
            .collect();
        let value = if steps.is_empty() {
            "no steps".to_owned()
        } else {
            format!("steps {}", steps.join(", "))
        };
        let response = ui.interact(strip, Id::new(("generator-strip", name)), Sense::hover());
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Group);
            node.set_label(name.to_owned());
            node.set_value(value);
            node.set_bounds(accesskit::Rect {
                x0: strip.min.x.into(),
                y0: strip.min.y.into(),
                x1: strip.max.x.into(),
                y1: strip.max.y.into(),
            });
        });
        y + STRIP + 4.0
    }

    /// Rhythm: the step pattern above its controls, Accent Offset with two
    /// arrow buttons (R4, R7).
    fn rhythm(&self, ui: &mut Ui, mut y: f32, events: &mut Vec<UiEvent>) -> f32 {
        let steps = usize::try_from(self.whole("steps")).unwrap_or(1);
        let hits = rhythm_steps(
            steps,
            usize::try_from(self.whole("density")).unwrap_or(0),
            usize::try_from(self.whole("pattern")).unwrap_or(0),
            self.whole("shift"),
        );
        y = self.strip(ui, y, &hits, false, "Step pattern");
        for spec in &self.view.descriptor.parameters {
            if spec.id == "accent_offset" {
                y = self.offset(ui, y, events);
            } else {
                y = self.row(ui, y, &spec.id, events);
            }
        }
        y
    }

    /// Accent Offset: a number between two arrows, which move which notes
    /// take the accent (R4); they stop at the ends of its range.
    fn offset(&self, ui: &mut Ui, y: f32, events: &mut Vec<UiEvent>) -> f32 {
        let (_, spec) = self.spec("accent_offset");
        self.name_at(ui, y, &spec.name);
        let offset = self.whole("accent_offset");
        let last = (self.whole("density") - 1).max(0);
        let x = self.rect.left() + CONTROLS_AT;
        let at = |index: f32, width: f32| {
            Rect::from_min_size(pos2(x + index, y), vec2(width, CONTROL.y))
        };
        for (text, to, enabled, rect) in [
            ("<", offset - 1, offset > 0, at(0.0, ARROW)),
            (">", offset + 1, offset < last, at(CONTROL.x - ARROW, ARROW)),
        ] {
            let salt = format!("{}-offset-{text}", self.view.descriptor.id);
            if button(ui, self.theme, &salt, rect, text, enabled) {
                events.push(UiEvent::Invoke(set(
                    self.clip,
                    self.view.descriptor,
                    spec,
                    json!(to),
                )));
            }
        }
        let middle = at(ARROW, CONTROL.x - 2.0 * ARROW);
        ui.painter().text(
            middle.center(),
            Align2::CENTER_CENTER,
            offset.to_string(),
            self.theme.small_font(),
            self.theme.colors.text.0,
        );
        if ui.rect_contains_pointer(at(0.0, CONTROL.x)) {
            control_info(ui, spec);
        }
        y + ROW
    }

    /// Seed: the pitch-range slider above its controls (R8).
    fn seed(&self, ui: &mut Ui, mut y: f32, events: &mut Vec<UiEvent>) -> f32 {
        y = self.slider(ui, y, events);
        y = self.row(ui, y, "pitch_range", events);
        y = self.pair(ui, y, "Duration", ("duration_min", "duration_max"), events);
        y = self.pair(ui, y, "Velocity", ("velocity_min", "velocity_max"), events);
        y = self.row(ui, y, "voices", events);
        self.row(ui, y, "density", events)
    }

    /// The pitch range as a slider over the keys, its handles merging into
    /// one where they meet, in the scale's colour where the clip has a scale
    /// (R8). Dragging a handle moves that end; dragging the bar between them
    /// moves both, keeping the width (Key Track).
    fn slider(&self, ui: &mut Ui, y: f32, events: &mut Vec<UiEvent>) -> f32 {
        let (index, spec) = self
            .view
            .descriptor
            .parameters
            .iter()
            .enumerate()
            .find(|(_, spec)| matches!(spec.kind, ParamKind::PitchRange))
            .expect("Seed has a pitch range");
        let ParamValue::PitchRange { low, high } = self.view.values[index] else {
            return y;
        };
        let rect = Rect::from_min_size(pos2(self.left(), y), vec2(self.width(), CONTROL.y));
        let id = Id::new(("generator-slider", &self.view.descriptor.id));
        let response = ui.interact(rect, id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let key_at = |x: f32| {
            ((x - rect.left()) / rect.width() * 128.0 - 0.5)
                .round()
                .clamp(0.0, 127.0) as u8
        };
        let x_of = |key: u8| rect.left() + (f32::from(key) + 0.5) / 128.0 * rect.width();
        let make = |low: u8, high: u8| {
            set(
                self.clip,
                self.view.descriptor,
                spec,
                json!({ "low": low, "high": high }),
            )
        };
        let drag_id = id.with("drag");
        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            let grabbed = key_at(origin.x);
            // Which end the drag moves: the nearer handle, both from the
            // bar between them; the merged handles go the way they're pulled.
            let part = if low == high {
                Part::Merged
            } else if grabbed > low.saturating_add(1) && grabbed < high.saturating_sub(1) {
                Part::Both(grabbed)
            } else if grabbed.abs_diff(low) <= grabbed.abs_diff(high) {
                Part::Low
            } else {
                Part::High
            };
            ui.data_mut(|d| d.insert_temp(drag_id, (part, false)));
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some((part, _)) = ui.data(|d| d.get_temp::<(Part, bool)>(drag_id))
        {
            let key = key_at(pointer.x);
            let (part, (new_low, new_high)) = match part {
                Part::Merged if key > low => (Part::High, (low, key)),
                Part::Merged if key < low => (Part::Low, (key, high)),
                Part::Merged => (Part::Merged, (low, high)),
                Part::Low => (Part::Low, (key.min(high), high)),
                Part::High => (Part::High, (low, key.max(low))),
                Part::Both(from) => {
                    let width = high - low;
                    let start = (i32::from(low) + i32::from(key) - i32::from(from))
                        .clamp(0, 127 - i32::from(width)) as u8;
                    (Part::Both(from), (start, start + width))
                }
            };
            ui.data_mut(|d| d.insert_temp(drag_id, (part, true)));
            if (new_low, new_high) != (low, high) {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Update,
                    invocation: Some(make(new_low, new_high)),
                });
            }
        }
        if response.drag_stopped() {
            let ran = ui
                .data_mut(|d| d.remove_temp::<(Part, bool)>(drag_id))
                .is_some_and(|(_, ran)| ran);
            end_gesture(ui, ran, events);
        }
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            // A click puts the nearer end there.
            let key = key_at(pointer.x);
            let invocation = if low == high && key != low {
                if key > low {
                    make(low, key)
                } else {
                    make(key, high)
                }
            } else if key.abs_diff(low) <= key.abs_diff(high) {
                make(key.min(high), high)
            } else {
                make(low, key.max(low))
            };
            events.push(UiEvent::Invoke(invocation));
        }
        // With the keyboard, Left and Right move both ends a key, and with
        // Shift an octave.
        if response.has_focus() {
            let (left, right, shift) = ui.input(|i| {
                (
                    i.key_pressed(Key::ArrowLeft),
                    i.key_pressed(Key::ArrowRight),
                    i.modifiers.shift,
                )
            });
            let by = i32::from(right) - i32::from(left);
            if by != 0 {
                let by = by * if shift { 12 } else { 1 };
                let width = i32::from(high - low);
                let start = (i32::from(low) + by).clamp(0, 127 - width) as u8;
                events.push(UiEvent::Invoke(make(start, start + width as u8)));
            }
        }
        // The slider: the scale's keys tinted, the range filled, a handle at
        // each end or one where they meet.
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(3.0));
        painter.rect_filled(rect, self.theme.sizes.corner, c.control.0);
        if let Some(scale) = self
            .shell
            .editor()
            .document()
            .clip(self.clip)
            .map(|clip| &clip.scale)
            .filter(|scale| scale.enabled)
        {
            for key in
                (0..=127u8).filter(|key| bl_core::scale::holds(scale.root, scale.steps(), *key))
            {
                let tint = Rect::from_x_y_ranges(
                    x_of(key) - rect.width() / 256.0..=x_of(key) + rect.width() / 256.0,
                    rect.y_range(),
                );
                painter.rect_filled(tint, 0.0, c.accent.0.gamma_multiply(0.18));
            }
        }
        let span = Rect::from_x_y_ranges(x_of(low)..=x_of(high), rect.y_range());
        painter.rect_filled(span, 0.0, c.accent.0.gamma_multiply(0.45));
        for key in if low == high {
            vec![low]
        } else {
            vec![low, high]
        } {
            let handle =
                Rect::from_center_size(pos2(x_of(key), rect.center().y), vec2(4.0, rect.height()));
            painter.rect_filled(handle, 1.0, c.accent.0);
        }
        if response.has_focus() && crate::focus::outlined(&response.ctx) {
            painter.rect_stroke(
                rect,
                self.theme.sizes.corner,
                Stroke::new(1.5, c.focus.0),
                StrokeKind::Outside,
            );
        }
        let name = format!("{} slider", spec.name);
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Slider, true, &name));
        if ui.rect_contains_pointer(rect) {
            control_info(ui, spec);
        }
        y + ROW
    }

    /// Shape: the preset chooser, the Shape Levels display, then the rest
    /// (R11).
    fn shape(&self, ui: &mut Ui, mut y: f32, events: &mut Vec<UiEvent>) -> f32 {
        y = self.row(ui, y, "preset", events);
        y = self.levels(ui, y, events);
        for id in ["pitch_range", "rate", "tie", "density", "jitter"] {
            y = self.row(ui, y, id, events);
        }
        y
    }

    /// The Shape Levels display: a column for each level, drawn by pressing or
    /// dragging across it, which sets the levels under the pointer and makes
    /// the preset read User (R11). With the keyboard, Left and Right choose a
    /// level and Up and Down change it.
    fn levels(&self, ui: &mut Ui, y: f32, events: &mut Vec<UiEvent>) -> f32 {
        let (index, spec) = self.spec("levels");
        let ParamValue::Levels(levels) = &self.view.values[index] else {
            return y;
        };
        let rect = Rect::from_min_size(pos2(self.left(), y), vec2(self.width(), LEVELS_HEIGHT));
        let id = Id::new(("generator-levels", &self.view.descriptor.id));
        let response = ui.interact(rect, id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let count = levels.len().max(1);
        let each = rect.width() / count as f32;
        let column = |x: f32| (((x - rect.left()) / each).floor().max(0.0) as usize).min(count - 1);
        let level = |y: f32| f64::from(((rect.bottom() - y) / rect.height()).clamp(0.0, 1.0));
        let make = |levels: &[f64]| set(self.clip, self.view.descriptor, spec, json!(levels));
        // Drawing sets the column under the pointer, and those between it
        // and the last one drawn, so a fast drag leaves no gaps.
        let draw = |levels: &mut Vec<f64>, last: Option<(usize, f64)>, pos: egui::Pos2| {
            let (at, now) = (column(pos.x), level(pos.y));
            let (from, begin) = last.unwrap_or((at, now));
            let (lo, hi) = (from.min(at), from.max(at));
            for (c, level) in levels.iter_mut().enumerate().take(hi + 1).skip(lo) {
                *level = if hi == lo {
                    now
                } else {
                    begin + (now - begin) * (c as f64 - from as f64) / (at as f64 - from as f64)
                };
            }
            (at, now)
        };
        let drag_id = id.with("drag");
        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            let mut drawn = levels.clone();
            let last = draw(&mut drawn, None, origin);
            ui.data_mut(|d| d.insert_temp(drag_id, (drawn, last, false)));
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some((mut drawn, last, _)) =
                ui.data(|d| d.get_temp::<(Vec<f64>, (usize, f64), bool)>(drag_id))
        {
            let last = draw(&mut drawn, Some(last), pointer);
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Update,
                invocation: Some(make(&drawn)),
            });
            ui.data_mut(|d| d.insert_temp(drag_id, (drawn, last, true)));
        }
        if response.drag_stopped() {
            let ran = ui
                .data_mut(|d| d.remove_temp::<(Vec<f64>, (usize, f64), bool)>(drag_id))
                .is_some_and(|(_, _, ran)| ran);
            end_gesture(ui, ran, events);
        }
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let mut drawn = levels.clone();
            draw(&mut drawn, None, pointer);
            events.push(UiEvent::Invoke(make(&drawn)));
        }
        let cursor_id = id.with("cursor");
        let mut cursor = ui
            .data(|d| d.get_temp::<usize>(cursor_id))
            .unwrap_or(0)
            .min(count - 1);
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
            let (left, right, up, down) = ui.input(|i| {
                (
                    i.key_pressed(Key::ArrowLeft),
                    i.key_pressed(Key::ArrowRight),
                    i.key_pressed(Key::ArrowUp),
                    i.key_pressed(Key::ArrowDown),
                )
            });
            cursor = (cursor + usize::from(right))
                .saturating_sub(usize::from(left))
                .min(count - 1);
            ui.data_mut(|d| d.insert_temp(cursor_id, cursor));
            let by = f64::from(i32::from(up) - i32::from(down)) * LEVEL_STEP;
            if by != 0.0 {
                let mut next = levels.clone();
                next[cursor] = (next[cursor] + by).clamp(0.0, 1.0);
                events.push(UiEvent::Invoke(make(&next)));
            }
        }
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(3.0));
        painter.rect_filled(rect, self.theme.sizes.corner, c.control.0);
        for (at, value) in levels.iter().enumerate() {
            let height = (*value as f32 * rect.height()).max(1.0);
            let bar = Rect::from_min_max(
                pos2(rect.left() + each * at as f32 + 0.5, rect.bottom() - height),
                pos2(rect.left() + each * (at + 1) as f32 - 0.5, rect.bottom()),
            );
            painter.rect_filled(bar, 0.0, c.accent.0.gamma_multiply(0.8));
        }
        if response.has_focus() && crate::focus::outlined(&response.ctx) {
            let column = Rect::from_min_size(
                pos2(rect.left() + each * cursor as f32, rect.top()),
                vec2(each, rect.height()),
            );
            painter.rect_stroke(column, 0.0, Stroke::new(1.5, c.focus.0), StrokeKind::Inside);
        }
        let (name, value) = (
            spec.name.clone(),
            levels.get(cursor).copied().unwrap_or(0.0),
        );
        response.widget_info(|| WidgetInfo::slider(true, value, &name));
        if ui.rect_contains_pointer(rect) {
            control_info(ui, spec);
        }
        y + LEVELS_HEIGHT + 4.0
    }

    /// Euclidean: a Pattern tab and a Voices tab (R14 to R16).
    fn euclidean(&self, ui: &mut Ui, mut y: f32, events: &mut Vec<UiEvent>) -> f32 {
        let tab_id = Id::new("euclidean-tab");
        let tab = ui.data(|d| d.get_temp::<usize>(tab_id)).unwrap_or(0);
        let half = self.width() / 2.0;
        for (index, label) in ["Pattern", "Voices"].into_iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(self.left() + half * index as f32, y),
                vec2(half - 2.0, CONTROL.y),
            );
            let id = Id::new(("euclidean-tab-button", index));
            let response = ui.interact(rect, id, Sense::click());
            crate::focus::register(ui.ctx(), id, Area::Detail, None);
            let c = &self.theme.colors;
            let shown = index == tab;
            ui.painter().rect_filled(
                rect,
                self.theme.sizes.corner,
                if shown {
                    c.control_pressed.0
                } else if response.hovered() {
                    c.control_hover.0
                } else {
                    c.control.0
                },
            );
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                self.theme.small_font(),
                if shown { c.text.0 } else { c.text_dim.0 },
            );
            response
                .widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, shown, label));
            if response.clicked()
                || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
            {
                ui.data_mut(|d| d.insert_temp(tab_id, index));
            }
        }
        y += ROW + 2.0;
        if tab == 0 {
            self.pattern_tab(ui, y, events)
        } else {
            self.voices_tab(ui, y, events)
        }
    }

    /// The voice's controls named by `prefix`, such as `voice` for its switch.
    fn voice_id(prefix: &str, voice: usize) -> String {
        format!("{prefix}_{}", voice + 1)
    }

    /// The Pattern tab: Steps, Density and Division, then each voice's
    /// switch, Rotation and the pattern it draws, and the button that sets
    /// every Rotation at random (R14, R16).
    fn pattern_tab(&self, ui: &mut Ui, mut y: f32, events: &mut Vec<UiEvent>) -> f32 {
        for id in ["steps", "density", "division"] {
            y = self.row(ui, y, id, events);
        }
        let steps = usize::try_from(self.whole("steps")).unwrap_or(1);
        let density = usize::try_from(self.whole("density")).unwrap_or(0);
        for voice in 0..EUCLIDEAN_VOICES {
            self.name_at(ui, y, &format!("Voice {}", voice + 1));
            let x = self.rect.left() + CONTROLS_AT;
            let switch = Self::voice_id("voice", voice);
            self.control(
                ui,
                &switch,
                Rect::from_min_size(pos2(x, y), vec2(SWITCH, CONTROL.y)),
                events,
            );
            let rotation = rotation_id(voice);
            self.control(
                ui,
                &rotation,
                Rect::from_min_size(pos2(x + SWITCH + 4.0, y), vec2(SMALL, CONTROL.y)),
                events,
            );
            let on = matches!(self.value(&switch), ParamValue::Switch(true));
            let hits = euclid(
                steps,
                density,
                usize::try_from(self.whole(&rotation)).unwrap_or(0),
            );
            y = self.strip(
                ui,
                y + CONTROL.y + 2.0,
                &hits,
                !on,
                &format!("Voice {} pattern", voice + 1),
            );
        }
        let rect = Rect::from_min_size(pos2(self.left(), y), vec2(self.width(), CONTROL.y + 2.0));
        if button(
            ui,
            self.theme,
            "randomize-euclidean",
            rect,
            "Randomize Rotations",
            true,
        ) {
            events.push(UiEvent::Invoke(Invocation::new(
                bl_shell::midi_tool_commands::RANDOMIZE,
                json!({ "clip": self.clip }),
            )));
        }
        y + CONTROL.y + 6.0
    }

    /// Stacks: the Chord Selector Pad with Add Chord and Delete Chord beside
    /// it, the progression's cells, then the selected chord's controls (R17,
    /// R21, R22).
    fn stacks(&self, ui: &mut Ui, mut y: f32, events: &mut Vec<UiEvent>) -> f32 {
        let Ok(stack) = stack_view(&self.view.values) else {
            return y;
        };
        let root = stack.chords[stack.selected].root;
        let entries = bl_shell::midi_tools::bank_chords(
            self.shell.editor().document(),
            self.shell.clip_view().midi_tools(),
            self.clip,
            root,
        );
        y = self.pad(ui, y, &stack, &entries, events);
        y = self.progression(ui, y, &stack, &entries, events);
        for id in [
            "chord_root",
            "chord_inversion",
            "chord_duration",
            "chord_offset",
        ] {
            y = self.row(ui, y, id, events);
        }
        y
    }

    /// The Chord Selector Pad: the bank's entries as Tonnetz diagrams, a
    /// page of them at a time, the selected chord's entry lit. A click or a
    /// drag chooses an entry; with the pad focused, Cmd+Up and Cmd+Down
    /// choose the one before or after (S-NOTE-052). While the pointer is over
    /// one, the Status Bar names it and its notes. Add Chord and Delete Chord
    /// are beside it, with the arrows that turn its pages (R17, R21).
    fn pad(
        &self,
        ui: &mut Ui,
        y: f32,
        stack: &StackView,
        entries: &[(String, Vec<u8>)],
        events: &mut Vec<UiEvent>,
    ) -> f32 {
        let (_, spec) = self.spec("chord");
        let current = stack.chords[stack.selected].chord;
        let per_page = PAD_COLUMNS * PAD_ROWS;
        // The page the selected entry is on, unless the arrows turned it.
        let page_id = Id::new(("chord-selector-page", &self.view.descriptor.id));
        let pages = entries.len().div_ceil(per_page).max(1);
        let following = current / per_page;
        let seen = ui.data(|d| d.get_temp::<(usize, usize)>(page_id));
        let page = match seen {
            Some((page, chord)) if chord == current => page.min(pages - 1),
            _ => following,
        };
        ui.data_mut(|d| d.insert_temp(page_id, (page, current)));
        let first = page * per_page;
        let step = vec2(DIAGRAM.x + DIAGRAM_GAP, DIAGRAM.y + DIAGRAM_GAP);
        let pad = Rect::from_min_size(
            pos2(self.left(), y),
            vec2(
                step.x * PAD_COLUMNS as f32 - DIAGRAM_GAP,
                step.y * PAD_ROWS as f32 - DIAGRAM_GAP,
            ),
        );
        let id = Id::new(("chord-selector-pad", &self.view.descriptor.id));
        let response = ui.interact(pad, id, Sense::click_and_drag());
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let cell_at = |pos: Pos2| -> Option<usize> {
            if !pad.contains(pos) {
                return None;
            }
            let column = ((pos.x - pad.left()) / step.x) as usize;
            let row = ((pos.y - pad.top()) / step.y) as usize;
            let at = first + row * PAD_COLUMNS + column.min(PAD_COLUMNS - 1);
            (at < entries.len()).then_some(at)
        };
        let choose = |entry: usize| set(self.clip, self.view.descriptor, spec, json!(entry));
        // The pad's keys, Cmd+Up and Cmd+Down among them, are the pad's while
        // it has the keyboard, not the note editor's (S-NOTE-052, S-NOTE-027).
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        if response.gained_focus() {
            events.push(UiEvent::FocusValue(true));
        }
        if response.lost_focus() {
            events.push(UiEvent::FocusValue(false));
        }
        let drag_id = id.with("drag");
        if response.drag_started() {
            ui.data_mut(|d| d.insert_temp(drag_id, false));
        }
        if response.dragged()
            && let Some(entry) = response.interact_pointer_pos().and_then(cell_at)
            && entry != current
        {
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Update,
                invocation: Some(choose(entry)),
            });
            ui.data_mut(|d| d.insert_temp(drag_id, true));
        }
        if response.drag_stopped() {
            let ran = ui
                .data_mut(|d| d.remove_temp::<bool>(drag_id))
                .unwrap_or(false);
            end_gesture(ui, ran, events);
        }
        if response.clicked()
            && let Some(entry) = response.interact_pointer_pos().and_then(cell_at)
            && entry != current
        {
            events.push(UiEvent::Invoke(choose(entry)));
        }
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
            let pressed = |wanted: Key| {
                ui.input(|i| {
                    i.events.iter().any(|event| {
                        matches!(event, egui::Event::Key { key, pressed: true, modifiers, .. }
                            if *key == wanted && modifiers.command && !modifiers.alt)
                    })
                })
            };
            let (before, after) = (pressed(Key::ArrowUp), pressed(Key::ArrowDown));
            let to = if before {
                current.checked_sub(1)
            } else if after {
                Some(current + 1).filter(|to| *to < entries.len())
            } else {
                None
            };
            if let Some(to) = to {
                events.push(UiEvent::Invoke(choose(to)));
            }
        }
        // While the pointer is over a diagram the Status Bar names the chord
        // and its notes on the selected chord's root.
        let hint_id = id.with("hint");
        let hovered = response.hover_pos().and_then(cell_at);
        let hinted = ui.data(|d| d.get_temp::<bool>(hint_id)).unwrap_or(false);
        if let Some(entry) = hovered {
            let (name, keys) = &entries[entry];
            let notes: Vec<String> = keys.iter().map(|key| key_name(*key)).collect();
            events.push(UiEvent::StatusHint(Some(format!(
                "{name}: {}",
                notes.join(" ")
            ))));
        } else if hinted {
            events.push(UiEvent::StatusHint(None));
        }
        ui.data_mut(|d| d.insert_temp(hint_id, hovered.is_some()));
        let c = &self.theme.colors;
        for (at, (_, keys)) in entries.iter().enumerate().skip(first).take(per_page) {
            let slot = at - first;
            let cell = Rect::from_min_size(
                pad.min
                    + vec2(
                        (slot % PAD_COLUMNS) as f32 * step.x,
                        (slot / PAD_COLUMNS) as f32 * step.y,
                    ),
                DIAGRAM,
            );
            let lit = at == current;
            ui.painter().rect_filled(
                cell,
                self.theme.sizes.corner,
                if lit {
                    c.control_pressed.0
                } else if hovered == Some(at) {
                    c.control_hover.0
                } else {
                    c.control.0
                },
            );
            tonnetz(
                ui,
                cell,
                keys,
                stack.chords[stack.selected].root,
                lit,
                self.theme,
            );
        }
        if response.has_focus() && crate::focus::outlined(&response.ctx) {
            ui.painter().rect_stroke(
                pad.expand(2.0),
                self.theme.sizes.corner,
                Stroke::new(1.5, c.focus.0),
                StrokeKind::Outside,
            );
        }
        let name = entries
            .get(current)
            .map_or_else(String::new, |(name, _)| name.clone());
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::ListBox);
            node.set_label("Chord Selector Pad".to_owned());
            node.set_value(name);
            node.set_bounds(accesskit::Rect {
                x0: pad.min.x.into(),
                y0: pad.min.y.into(),
                x1: pad.max.x.into(),
                y1: pad.max.y.into(),
            });
        });
        if ui.rect_contains_pointer(pad) {
            control_info(ui, spec);
        }
        // Add Chord and Delete Chord, and the page arrows, right of the pad.
        let x = pad.right() + 6.0;
        let width = (self.rect.right() - 6.0 - x).max(40.0);
        let invoke = |id: &str| UiEvent::Invoke(Invocation::new(id, json!({ "clip": self.clip })));
        let at = |row: f32, left: f32, width: f32| {
            Rect::from_min_size(pos2(left, y + row * ROW), vec2(width, CONTROL.y))
        };
        if button(
            ui,
            self.theme,
            "stacks-add-chord",
            at(0.0, x, width),
            "Add Chord",
            stack.chords.len() < MAX_CHORDS,
        ) {
            events.push(invoke(bl_shell::midi_tool_commands::ADD_CHORD));
        }
        if button(
            ui,
            self.theme,
            "stacks-delete-chord",
            at(1.0, x, width),
            "Delete Chord",
            stack.chords.len() > 1,
        ) {
            events.push(invoke(bl_shell::midi_tool_commands::DELETE_CHORD));
        }
        for (index, (text, to, enabled)) in [
            ("<", page.wrapping_sub(1), page > 0),
            (">", page + 1, page + 1 < pages),
        ]
        .into_iter()
        .enumerate()
        {
            let half = (width - 4.0) / 2.0;
            if button(
                ui,
                self.theme,
                &format!("stacks-page-{text}"),
                at(2.0, x + (half + 4.0) * index as f32, half),
                text,
                enabled,
            ) {
                ui.data_mut(|d| d.insert_temp(page_id, (to, current)));
            }
        }
        y + pad.height() + 4.0
    }

    /// The progression as a row of cells, the selected chord's lit; clicking
    /// one selects that chord (R17).
    fn progression(
        &self,
        ui: &mut Ui,
        y: f32,
        stack: &StackView,
        entries: &[(String, Vec<u8>)],
        events: &mut Vec<UiEvent>,
    ) -> f32 {
        let (_, spec) = self.spec("selected");
        let gap = 1.0;
        let count = stack.chords.len();
        let each = ((self.width() + gap) / count as f32 - gap).min(18.0);
        let c = &self.theme.colors;
        for (at, chord) in stack.chords.iter().enumerate() {
            let cell = Rect::from_min_size(
                pos2(self.left() + (each + gap) * at as f32, y),
                vec2(each, CONTROL.y),
            );
            let id = Id::new(("progression-cell", &self.view.descriptor.id, at));
            let response = ui.interact(cell, id, Sense::click());
            crate::focus::register(ui.ctx(), id, Area::Detail, None);
            let selected = at == stack.selected;
            ui.painter().rect_filled(
                cell,
                self.theme.sizes.corner,
                if selected {
                    c.accent.0
                } else if response.hovered() {
                    c.control_hover.0
                } else {
                    c.control.0
                },
            );
            ui.painter().with_clip_rect(cell).text(
                cell.center(),
                Align2::CENTER_CENTER,
                (at + 1).to_string(),
                self.theme.small_font(),
                if selected { c.text.0 } else { c.text_dim.0 },
            );
            let label = format!(
                "Chord {}: {} on {}",
                at + 1,
                entries
                    .get(chord.chord)
                    .map_or("", |(name, _)| name.as_str()),
                key_name(chord.root)
            );
            response.widget_info(|| {
                WidgetInfo::selected(WidgetType::RadioButton, true, selected, &label)
            });
            if (response.clicked()
                || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))))
                && !selected
            {
                events.push(UiEvent::Invoke(set(
                    self.clip,
                    self.view.descriptor,
                    spec,
                    json!(at),
                )));
            }
            if response.hovered() {
                control_info(ui, spec);
            }
        }
        y + ROW + 2.0
    }

    /// The Voices tab: each voice's switch, pitch and Velocity, and the
    /// arrows that move every voice's pitch together (R14).
    fn voices_tab(&self, ui: &mut Ui, mut y: f32, events: &mut Vec<UiEvent>) -> f32 {
        for voice in 0..EUCLIDEAN_VOICES {
            self.name_at(ui, y, &format!("Voice {}", voice + 1));
            let x = self.rect.left() + CONTROLS_AT;
            let ids = ["voice", "pitch", "velocity"].map(|prefix| Self::voice_id(prefix, voice));
            for (id, at, width) in [
                (&ids[0], 0.0, SWITCH),
                (&ids[1], SWITCH + 4.0, SMALL),
                (&ids[2], SWITCH + SMALL + 8.0, SMALL),
            ] {
                self.control(
                    ui,
                    id,
                    Rect::from_min_size(pos2(x + at, y), vec2(width, CONTROL.y)),
                    events,
                );
            }
            y += ROW;
        }
        self.name_at(ui, y, "All Pitches");
        let pitches: Vec<u8> = (0..EUCLIDEAN_VOICES)
            .map(|voice| match self.value(&Self::voice_id("pitch", voice)) {
                ParamValue::Pitch(key) => *key,
                _ => 0,
            })
            .collect();
        let doc = self.shell.editor().document();
        for (index, (text, degrees)) in [("<", -1), (">", 1)].into_iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(
                    self.rect.left() + CONTROLS_AT + (ARROW + 4.0) * index as f32,
                    y,
                ),
                vec2(ARROW, CONTROL.y),
            );
            if button(
                ui,
                self.theme,
                &format!("euclidean-pitches-{text}"),
                rect,
                text,
                true,
            ) {
                let moved = bl_shell::midi_tools::stepped_keys(doc, self.clip, &pitches, degrees);
                let values: serde_json::Map<String, Value> = moved
                    .iter()
                    .enumerate()
                    .map(|(voice, key)| (Self::voice_id("pitch", voice), json!(key)))
                    .collect();
                events.push(UiEvent::Invoke(Invocation::new(
                    bl_shell::midi_tool_commands::SET,
                    json!({
                        "clip": self.clip,
                        "tool": self.view.descriptor.id,
                        "values": values,
                    }),
                )));
            }
        }
        y + ROW
    }
}

/// A chord as a Tonnetz diagram in `cell`: the twelve pitch classes on a
/// lattice of fifths across and major thirds up, relative to `root`, the
/// chord's notes lit and its root ringed. The drawing is Beatlayer's own.
fn tonnetz(ui: &Ui, cell: Rect, keys: &[u8], root: u8, selected: bool, theme: &Theme) {
    const COLUMNS: i32 = 4;
    const ROWS: i32 = 3;
    let c = &theme.colors;
    let inner = cell.shrink2(vec2(5.0, 4.0));
    let at = |a: i32, b: i32| {
        pos2(
            inner.left() + inner.width() * a as f32 / (COLUMNS - 1) as f32,
            inner.bottom() - inner.height() * b as f32 / (ROWS - 1) as f32,
        )
    };
    let lit: Vec<i32> = keys
        .iter()
        .map(|key| (i32::from(*key) - i32::from(root)).rem_euclid(12))
        .collect();
    let ink = if selected { c.text.0 } else { c.accent.0 };
    for a in 0..COLUMNS {
        for b in 0..ROWS {
            // Along the row a step is a fifth (7 semitones), up it is a
            // major third (4): the twelve places hold each class once.
            let class = (7 * a + 4 * b).rem_euclid(12);
            let place = at(a, b);
            if lit.contains(&class) {
                ui.painter().circle_filled(place, 2.2, ink);
                if class == 0 {
                    ui.painter()
                        .circle_stroke(place, 3.6, Stroke::new(1.0, ink));
                }
            } else {
                ui.painter()
                    .circle_filled(place, 0.8, c.text_dim.0.gamma_multiply(0.45));
            }
        }
    }
}

/// What a drag on Seed's slider moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Part {
    /// The low end.
    Low,
    /// The high end.
    High,
    /// Both ends, kept apart, grabbed at this key.
    Both(u8),
    /// Handles that met: whichever way the pointer pulls.
    #[default]
    Merged,
}
