//! The displays of Recombine, Strum, Time Warp and Velocity Shaper in the
//! Transform panel (WP-6.13; the MIDI transformations spec, R43).
//!
//! Each display is drawn from the tool's settings, and a gesture on it is a
//! gesture of `midi_tool.set`, so it is one undo step and Esc puts the notes
//! back. `midi_tool.state` reads what a display shows in the tool's values.
//! Their look and where they sit in the panel are Beatlayer's own.

use bl_midi_tools::{Breakpoint, ENVELOPE_POINTS, ParamSpec, ParamValue, ToolDescriptor};
use bl_model::{ClipId, Note};
use bl_shell::{Invocation, Shell};
use egui::accesskit::Role;
use egui::{Align2, Id, Pos2, Rect, Sense, Stroke, Ui, WidgetInfo, WidgetType, pos2, vec2};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::tool_panel::{CONTROL, CONTROLS_AT, ROW, Shown, button_named, control, drag_gesture};
use crate::widgets;

/// How tall a display is.
const DISPLAY: f32 = 52.0;
/// The space round a display's drawing, so a handle at an edge stays whole.
const PAD: f32 = 6.0;
/// How far a handle reaches.
const HANDLE: f32 = 4.0;
/// How many points the strum curve is drawn with.
const CURVE: usize = 24;

/// What a display took of the panel: the height it drew, and the parameters
/// whose controls it replaces.
pub(crate) struct Drawn {
    pub height: f32,
    pub hidden: Vec<&'static str>,
}

impl Drawn {
    fn none() -> Self {
        Self {
            height: 0.0,
            hidden: Vec::new(),
        }
    }
}

/// Draws the display of `tool`, if it has one, at `at` and `width` wide.
#[expect(clippy::too_many_arguments, reason = "a display's whole context")]
pub(crate) fn show(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    clip: ClipId,
    tool: &ToolDescriptor,
    values: &[ParamValue],
    at: Pos2,
    width: f32,
    events: &mut Vec<UiEvent>,
) -> Drawn {
    let place = Place {
        theme,
        clip,
        tool,
        values,
        at,
        width,
    };
    match tool.id.as_str() {
        "recombine" => recombine(ui, shell, &place, events),
        "strum" => strum(ui, &place, events),
        "time_warp" => time_warp(ui, &place, events),
        "velocity_shaper" => velocity_shaper(ui, &place, events),
        _ => Drawn::none(),
    }
}

/// Where a display goes and what it is drawn from.
struct Place<'a> {
    theme: &'a Theme,
    clip: ClipId,
    tool: &'a ToolDescriptor,
    values: &'a [ParamValue],
    at: Pos2,
    width: f32,
}

impl Place<'_> {
    fn rect(&self) -> Rect {
        Rect::from_min_size(self.at, vec2(self.width, DISPLAY))
    }

    fn whole(&self, id: &str) -> i64 {
        let value = self
            .tool
            .parameter(id)
            .and_then(|(at, _)| self.values.get(at));
        match value {
            Some(ParamValue::Whole(n)) => *n,
            _ => 0,
        }
    }

    fn on(&self, id: &str) -> bool {
        let value = self
            .tool
            .parameter(id)
            .and_then(|(at, _)| self.values.get(at));
        matches!(value, Some(ParamValue::Switch(true)))
    }

    fn spec(&self, id: &str) -> Option<(usize, &ParamSpec)> {
        self.tool.parameter(id)
    }

    /// The `midi_tool.set` for these values.
    fn set(&self, values: Value) -> Invocation {
        Invocation::new(
            bl_shell::midi_tool_commands::SET,
            json!({ "clip": self.clip, "tool": self.tool.id, "values": values }),
        )
    }

    /// The display's background.
    fn frame(&self, ui: &Ui) -> Rect {
        let rect = self.rect();
        let c = &self.theme.colors;
        ui.painter().rect_filled(
            rect,
            self.theme.sizes.corner,
            c.control.0.gamma_multiply(0.55),
        );
        rect
    }
}

/// A handle: its name for assistive technology, its value and how that
/// reads, and whether it is lit.
struct Handle<'a> {
    name: &'a str,
    value: f64,
    text: String,
    lit: bool,
}

/// A handle's response, drawn as a dot at `at`, as a slider for assistive
/// technology.
fn handle(ui: &mut Ui, place: &Place<'_>, at: Pos2, handle: Handle<'_>) -> egui::Response {
    let Handle {
        name,
        value,
        text,
        lit,
    } = handle;
    let id = Id::new(("tool-display-handle", place.tool.id.as_str(), name));
    let response = ui.interact(
        Rect::from_center_size(at, vec2(2.0 * HANDLE + 4.0, 2.0 * HANDLE + 4.0)),
        id,
        Sense::click_and_drag(),
    );
    let c = &place.theme.colors;
    let painter = ui.painter();
    painter.circle_filled(
        at,
        HANDLE,
        if lit || response.hovered() || response.dragged() {
            c.accent.0
        } else {
            c.text.0
        },
    );
    painter.circle_stroke(at, HANDLE, Stroke::new(1.0, c.panel.0));
    response.widget_info(|| WidgetInfo::slider(true, value, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Slider);
        node.set_label(name.to_owned());
        node.set_value(text);
        node.set_numeric_value(value);
    });
    response
}

/// A row's name, to the left of its control.
fn row_label(ui: &Ui, place: &Place<'_>, y: f32, text: &str) {
    ui.painter().text(
        pos2(place.at.x, y + CONTROL.y / 2.0),
        Align2::LEFT_CENTER,
        text,
        place.theme.small_font(),
        place.theme.colors.text_dim.0,
    );
}

fn percent(value: i64) -> String {
    format!("{value} %")
}

// Recombine: the chosen values as columns, dragged across to rotate.

fn recombine(ui: &mut Ui, shell: &Shell, place: &Place<'_>, events: &mut Vec<UiEvent>) -> Drawn {
    let (notes, _) = bl_shell::midi_tools::input_notes(
        shell.editor().document(),
        shell.clip_view(),
        shell.arrangement(),
        place.clip,
    );
    let dimension = place
        .spec("dimension")
        .and_then(|(at, _)| place.values.get(at))
        .map_or(0, |value| match value {
            ParamValue::Choice(n) => *n,
            _ => 0,
        });
    let value_of = |note: &Note| match dimension {
        0 => note.start.0 as f64,
        1 => f64::from(note.key),
        2 => note.length.0 as f64,
        _ => f64::from(note.velocity),
    };
    let rect = place.frame(ui);
    let c = &place.theme.colors;
    let count = notes.len();
    let steps = place.whole("rotation_steps");
    let columns: Vec<f64> = notes.iter().map(value_of).collect();
    let (low, high) = columns
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
    let inner = rect.shrink(2.0);
    let each = inner.width() / count.max(1) as f32;
    for (at, value) in columns.iter().enumerate() {
        let share = if high > low {
            0.1 + 0.9 * (value - low) / (high - low)
        } else {
            0.5
        };
        let column = Rect::from_min_max(
            pos2(
                inner.left() + each * at as f32 + 0.5,
                inner.bottom() - inner.height() * share as f32,
            ),
            pos2(inner.left() + each * (at + 1) as f32 - 0.5, inner.bottom()),
        );
        ui.painter()
            .rect_filled(column, 0.0, c.accent.0.gamma_multiply(0.7));
    }
    // Dragging across the columns sets Rotation Steps.
    let id = Id::new(("recombine-display", place.clip));
    let response = ui.interact(rect, id, Sense::drag());
    let most = count.saturating_sub(1) as i64;
    let base_id = id.with("steps");
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(base_id, steps));
    }
    let base = ui.data(|d| d.get_temp::<i64>(base_id)).unwrap_or(steps);
    if count > 1 {
        drag_gesture(ui, &response, events, |start, now, ran| {
            let moved = ((now.x - start.x) / each).round() as i64;
            let next = (base + moved).clamp(-most, most);
            (next != steps || ran).then(|| place.set(json!({ "rotation_steps": next })))
        });
    }
    response.widget_info(|| WidgetInfo::slider(count > 1, steps as f64, "Recombine Columns"));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Slider);
        node.set_label("Recombine Columns".to_owned());
        node.set_value(format!("{count} values, rotated {steps}"));
    });
    // The Rotate Step buttons lower and raise Rotation Steps by one.
    let y = rect.bottom() + 3.0;
    let half = (place.width - 4.0) / 2.0;
    for (index, (shown, name, delta)) in [
        ("\u{2039} Step", "Rotate Step Down", -1),
        ("Step \u{203a}", "Rotate Step Up", 1),
    ]
    .into_iter()
    .enumerate()
    {
        let button = Rect::from_min_size(
            pos2(place.at.x + (half + 4.0) * index as f32, y),
            vec2(half, CONTROL.y + 2.0),
        );
        let next = (steps + delta).clamp(-most, most);
        if button_named(
            ui,
            place.theme,
            &format!("recombine-{delta}"),
            button,
            shown,
            name,
            next != steps,
        ) {
            events.push(UiEvent::Invoke(
                place.set(json!({ "rotation_steps": next })),
            ));
        }
    }
    // Rotate on Grid is for Position alone.
    Drawn {
        height: DISPLAY + CONTROL.y + 10.0,
        hidden: if dimension == 0 {
            Vec::new()
        } else {
            vec!["rotate_on_grid"]
        },
    }
}

// Strum: the Strum Position display.

fn strum(ui: &mut Ui, place: &Place<'_>, events: &mut Vec<UiEvent>) -> Drawn {
    let rect = place.frame(ui);
    let c = &place.theme.colors;
    let (low, high, tension) = (
        place.whole("strum_low"),
        place.whole("strum_high"),
        place.whole("tension"),
    );
    let half = rect.height() / 2.0 - PAD;
    let x_of = |along: f64| rect.left() + PAD + along as f32 * (rect.width() - 2.0 * PAD);
    let y_of = |share: f64| rect.center().y - share as f32 * half;
    // The offset, as a share of the grid step, of the note `along` of the way
    // from the lowest to the highest.
    let offset = |j: usize| {
        let along = bl_midi_tools::strum_position(j, CURVE + 1, tension as f64 / 100.0);
        (low as f64 + (high - low) as f64 * along) / 100.0
    };
    ui.painter().line_segment(
        [
            pos2(rect.left() + PAD, y_of(0.0)),
            pos2(rect.right() - PAD, y_of(0.0)),
        ],
        Stroke::new(1.0, c.text_dim.0.gamma_multiply(0.5)),
    );
    let curve: Vec<Pos2> = (0..=CURVE)
        .map(|j| pos2(x_of(j as f64 / CURVE as f64), y_of(offset(j))))
        .collect();
    ui.painter()
        .add(egui::Shape::line(curve, Stroke::new(1.5, c.accent.0)));
    // The two breakpoints move their value up and down.
    let from_y = |y: f32| {
        (((rect.center().y - y) / half) as f64 * 100.0)
            .round()
            .clamp(-100.0, 100.0) as i64
    };
    for (parameter, name, along, value) in [
        ("strum_low", "Strum Low Breakpoint", 0.0, low),
        ("strum_high", "Strum High Breakpoint", 1.0, high),
    ] {
        let at = pos2(x_of(along), y_of(value as f64 / 100.0));
        let shown = Handle {
            name,
            value: value as f64,
            text: percent(value),
            lit: false,
        };
        let response = handle(ui, place, at, shown);
        drag_gesture(ui, &response, events, |_, now, ran| {
            let next = from_y(now.y);
            (next != value || ran).then(|| place.set(json!({ parameter: next })))
        });
    }
    // The curve's middle sets Tension: up bends it towards the higher end.
    let middle = pos2(x_of(0.5), y_of(offset(CURVE / 2)));
    let shown = Handle {
        name: "Tension Curve",
        value: tension as f64,
        text: percent(tension),
        lit: false,
    };
    let response = handle(ui, place, middle, shown);
    let start_id = response.id.with("tension");
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(start_id, tension));
    }
    let start = ui.data(|d| d.get_temp::<i64>(start_id)).unwrap_or(tension);
    let towards = if high < low { -1.0 } else { 1.0 };
    drag_gesture(ui, &response, events, |from, now, ran| {
        let moved = f64::from((from.y - now.y) / half) * 100.0 * towards;
        let next = (start as f64 + moved).round().clamp(-100.0, 100.0) as i64;
        (next != tension || ran).then(|| place.set(json!({ "tension": next })))
    });
    Drawn {
        height: DISPLAY + 4.0,
        hidden: Vec::new(),
    }
}

// Time Warp: the speed curve with three breakpoints.

/// The speed curve's range: 25 % to 400 %, 100 % in the middle.
const SPEEDS: (f64, f64) = (25.0, 400.0);

fn selected_breakpoint(ui: &Ui, clip: ClipId) -> usize {
    ui.data(|d| d.get_temp::<usize>(Id::new(("time-warp-breakpoint", clip))))
        .unwrap_or(0)
}

fn time_warp(ui: &mut Ui, place: &Place<'_>, events: &mut Vec<UiEvent>) -> Drawn {
    let rect = place.frame(ui);
    let c = &place.theme.colors;
    let selected = selected_breakpoint(ui, place.clip);
    let name = |n: usize, what: &str| format!("breakpoint_{}{what}", n + 1);
    let points: Vec<(usize, i64, i64)> = (0..3)
        .filter(|n| place.on(&name(*n, "")))
        .map(|n| {
            (
                n,
                place.whole(&name(n, "_time")),
                place.whole(&name(n, "_speed")),
            )
        })
        .collect();
    let span = (SPEEDS.1 / SPEEDS.0).log2();
    let x_of = |time: i64| rect.left() + PAD + time as f32 / 100.0 * (rect.width() - 2.0 * PAD);
    let y_of = |speed: i64| {
        let along = ((speed as f64 / SPEEDS.0).log2() / span) as f32;
        rect.bottom() - PAD - along * (rect.height() - 2.0 * PAD)
    };
    let speed_at = |y: f32| {
        let along = f64::from((rect.bottom() - PAD - y) / (rect.height() - 2.0 * PAD));
        (SPEEDS.0 * 2f64.powf(along.clamp(0.0, 1.0) * span))
            .round()
            .clamp(SPEEDS.0, SPEEDS.1) as i64
    };
    let time_at = |x: f32| {
        (f64::from((x - rect.left() - PAD) / (rect.width() - 2.0 * PAD)) * 100.0)
            .round()
            .clamp(0.0, 100.0) as i64
    };
    // 100 % is the line across the middle; the curve is level before the first
    // breakpoint and after the last (R32).
    ui.painter().line_segment(
        [
            pos2(rect.left() + PAD, y_of(100)),
            pos2(rect.right() - PAD, y_of(100)),
        ],
        Stroke::new(1.0, c.text_dim.0.gamma_multiply(0.5)),
    );
    let mut sorted = points.clone();
    sorted.sort_by_key(|(_, time, _)| *time);
    if let (Some(first), Some(last)) = (sorted.first(), sorted.last()) {
        let mut line = vec![pos2(rect.left() + PAD, y_of(first.2))];
        line.extend(
            sorted
                .iter()
                .map(|(_, time, speed)| pos2(x_of(*time), y_of(*speed))),
        );
        line.push(pos2(rect.right() - PAD, y_of(last.2)));
        ui.painter()
            .add(egui::Shape::line(line, Stroke::new(1.5, c.accent.0)));
    }
    for (n, time, speed) in &points {
        let label = format!("Speed Curve Breakpoint {}", n + 1);
        let shown = Handle {
            name: &label,
            value: *speed as f64,
            text: format!("{time} % of the range at {speed} %"),
            lit: *n == selected,
        };
        let response = handle(ui, place, pos2(x_of(*time), y_of(*speed)), shown);
        if response.drag_started() || response.clicked() {
            ui.data_mut(|d| d.insert_temp(Id::new(("time-warp-breakpoint", place.clip)), *n));
        }
        let (time_id, speed_id) = (name(*n, "_time"), name(*n, "_speed"));
        drag_gesture(ui, &response, events, |_, now, ran| {
            let (next_time, next_speed) = (time_at(now.x), speed_at(now.y));
            ((next_time, next_speed) != (*time, *speed) || ran)
                .then(|| place.set(json!({ time_id: next_time, speed_id: next_speed })))
        });
    }
    // The breakpoints' buttons select one, and its switch, Time and Speed
    // follow below.
    let mut y = rect.bottom() + 3.0;
    let each = (place.width - 8.0) / 3.0;
    for n in 0..3 {
        let button = Rect::from_min_size(
            pos2(place.at.x + (each + 4.0) * n as f32, y),
            vec2(each, CONTROL.y),
        );
        let id = Id::new(("time-warp-select", place.clip, n));
        let response = ui.interact(button, id, Sense::click());
        let on = place.on(&name(n, ""));
        ui.painter().rect_filled(
            button,
            place.theme.sizes.corner,
            if n == selected {
                c.control_pressed.0
            } else if response.hovered() {
                c.control_hover.0
            } else {
                c.control.0
            },
        );
        ui.painter().text(
            button.center(),
            Align2::CENTER_CENTER,
            (n + 1).to_string(),
            place.theme.small_font(),
            if on { c.text.0 } else { c.text_dim.0 },
        );
        let label = format!("Breakpoint {}", n + 1);
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::RadioButton, true, n == selected, &label)
        });
        crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
        if response.clicked() {
            ui.data_mut(|d| d.insert_temp(Id::new(("time-warp-breakpoint", place.clip)), n));
        }
    }
    y += ROW;
    // The selected breakpoint's switch, Time and Speed. The last one on
    // can't be switched off (R31).
    let n = selected;
    let on = place.on(&name(n, ""));
    row_label(ui, place, y, "Breakpoint");
    let switch = Rect::from_min_size(
        pos2(place.at.x + CONTROLS_AT - 6.0, y),
        vec2(40.0, CONTROL.y),
    );
    crate::focus::register(
        ui.ctx(),
        Id::new(("switch", format!("time-warp-breakpoint-{n}"))),
        bl_shell::Area::Detail,
        None,
    );
    if let Some(next) = widgets::switch(
        ui,
        place.theme,
        &format!("time-warp-breakpoint-{n}"),
        "Breakpoint",
        switch,
        on,
    ) && (next || points.len() > 1)
    {
        events.push(UiEvent::Invoke(place.set(json!({ name(n, ""): next }))));
    }
    y += ROW;
    for (label, what) in [("Breakpoint Time", "_time"), ("Breakpoint Speed", "_speed")] {
        row_label(ui, place, y, label);
        if let Some((at, spec)) = place.spec(&name(n, what)) {
            let mut spec = spec.clone();
            spec.name = label.to_owned();
            if let Some(value) = place.values.get(at) {
                let field = Rect::from_min_size(pos2(place.at.x + CONTROLS_AT - 6.0, y), CONTROL);
                control(
                    ui,
                    place.theme,
                    place.clip,
                    place.tool,
                    &spec,
                    value,
                    Shown {
                        at: field,
                        text: None,
                        enabled: true,
                    },
                    events,
                );
            }
        }
        y += ROW;
    }
    Drawn {
        height: y - place.at.y + 4.0,
        hidden: vec![
            "breakpoint_1",
            "breakpoint_1_time",
            "breakpoint_1_speed",
            "breakpoint_2",
            "breakpoint_2_time",
            "breakpoint_2_speed",
            "breakpoint_3",
            "breakpoint_3_time",
            "breakpoint_3_speed",
        ],
    }
}

// Velocity Shaper: the envelope, clicked to add breakpoints and dragged.

/// How close two breakpoints' times may come.
const GAP: f64 = 0.01;

fn round_to_hundredths(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn velocity_shaper(ui: &mut Ui, place: &Place<'_>, events: &mut Vec<UiEvent>) -> Drawn {
    let rect = place.frame(ui);
    let c = &place.theme.colors;
    let points: Vec<Breakpoint> = place
        .spec("envelope")
        .and_then(|(at, _)| place.values.get(at))
        .and_then(|value| match value {
            ParamValue::Breakpoints(points) => Some(points.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let width = rect.width() - 2.0 * PAD;
    let height = rect.height() - 2.0 * PAD;
    let at = |point: &Breakpoint| {
        pos2(
            rect.left() + PAD + point.x as f32 * width,
            rect.bottom() - PAD - point.y as f32 * height,
        )
    };
    let envelope = |points: &[Breakpoint]| json!({ "envelope": points.iter().map(|p| json!({ "x": p.x, "y": p.y })).collect::<Vec<_>>() });
    // Clicking where there is no breakpoint adds one.
    let id = Id::new(("velocity-shaper-envelope", place.clip));
    let background = ui.interact(rect, id, Sense::click());
    background
        .widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Velocity Shaper Envelope"));
    ui.painter().add(egui::Shape::line(
        points.iter().map(at).collect(),
        Stroke::new(1.5, c.accent.0),
    ));
    let last = points.len().saturating_sub(1);
    for (index, point) in points.iter().enumerate() {
        let name = format!("Envelope Breakpoint {}", index + 1);
        let shown = Handle {
            name: &name,
            value: point.y,
            text: format!(
                "{:.0} % of the way, at {:.0} %",
                point.x * 100.0,
                point.y * 100.0
            ),
            lit: false,
        };
        let response = handle(ui, place, at(point), shown);
        drag_gesture(ui, &response, events, |_, now, ran| {
            let y = round_to_hundredths(f64::from((rect.bottom() - PAD - now.y) / height))
                .clamp(0.0, 1.0);
            // The first stays at the start and the last at the end, and one
            // with no room between its neighbours stays where it is.
            let (lowest, highest) = if index == 0 || index == last {
                (point.x, point.x)
            } else {
                (points[index - 1].x + GAP, points[index + 1].x - GAP)
            };
            let x = if lowest < highest {
                round_to_hundredths(f64::from((now.x - rect.left() - PAD) / width))
                    .clamp(lowest, highest)
            } else {
                point.x
            };
            ((x, y) != (point.x, point.y) || ran).then(|| {
                let mut next = points.clone();
                next[index] = Breakpoint { x, y };
                place.set(envelope(&next))
            })
        });
    }
    if background.clicked()
        && let Some(pos) = background.interact_pointer_pos()
        && points.len() < ENVELOPE_POINTS.1
    {
        let x = round_to_hundredths(f64::from((pos.x - rect.left() - PAD) / width)).clamp(0.0, 1.0);
        let y =
            round_to_hundredths(f64::from((rect.bottom() - PAD - pos.y) / height)).clamp(0.0, 1.0);
        // Not on top of a breakpoint, which a click on its handle took.
        if points.iter().all(|p| (p.x - x).abs() >= GAP) {
            let mut next = points.clone();
            next.push(Breakpoint { x, y });
            next.sort_by(|a, b| a.x.total_cmp(&b.x));
            events.push(UiEvent::Invoke(place.set(envelope(&next))));
        }
    }
    Drawn {
        height: DISPLAY + 4.0,
        hidden: vec!["envelope"],
    }
}
