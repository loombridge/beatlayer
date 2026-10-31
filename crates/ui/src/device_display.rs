//! What a device's display cell draws from its display data (the device
//! framework spec, R36): spectra and curves over their axes with markers on
//! them, a gain reduction meter, and the views a display offers. The data is
//! the device type's own (`bl_model::DeviceDisplay`); what is drawn depends
//! only on what the data holds, so a later device draws through the same
//! code.

use bl_model::{DeviceDisplay, DeviceId, DisplaySeries};
use bl_shell::Invocation;
use bl_shell::device_commands::DevicePanel;
use egui::accesskit::Role;
use egui::{Align2, Color32, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The series a display draws as a filled spectrum.
const SPECTRUM: &str = "Spectrum";
/// The reading a display draws as a meter down its right edge.
pub const GAIN_REDUCTION: &str = "Gain Reduction";
/// The reading whose history the Activity view draws.
const INPUT: &str = "Input";
/// The views whose names the display draws specially.
const COLLAPSED: &str = "Collapsed";
const ACTIVITY: &str = "Activity";
/// The meter's width, and what it shows between its top and its bottom, in
/// dB of reduction.
const METER_WIDTH: f32 = 6.0;
const METER_RANGE_DB: f32 = 24.0;
/// How many frames the Activity view remembers.
const HISTORY: usize = 240;
/// The view buttons' size, in points.
const VIEW_BUTTON: egui::Vec2 = vec2(26.0, 12.0);

/// The ID of a view button in a device's panel.
#[must_use]
pub fn view_button_id(device: DeviceId, view: &str) -> Id {
    Id::new(("device-view", device, view.to_owned()))
}

/// The short text on a view's button.
fn view_label(view: &str) -> &str {
    match view {
        "Transfer Curve" => "Curve",
        ACTIVITY => "Act",
        COLLAPSED => "Min",
        other => other,
    }
}

/// Where `x` on `series`' axis sits across `rect`, from 0 to 1.
fn across(series: &DisplaySeries, x: f32) -> f32 {
    let [from, to] = series.x_range;
    if series.log_x {
        (x / from).max(f32::MIN_POSITIVE).ln() / (to / from).ln()
    } else {
        (x - from) / (to - from)
    }
}

/// Where `y` on `series`' scale sits up `rect`, from 0 at the bottom to 1.
fn up(series: &DisplaySeries, y: f32) -> f32 {
    let [from, to] = series.y_range;
    ((y - from) / (to - from)).clamp(0.0, 1.0)
}

fn point(rect: Rect, series: &DisplaySeries, x: f32, y: f32) -> Pos2 {
    pos2(
        rect.left() + rect.width() * across(series, x),
        rect.bottom() - rect.height() * up(series, y),
    )
}

/// What the Activity view remembers: the input's level and the gain
/// reduction, one pair per frame drawn.
#[derive(Clone, Default)]
struct History(Vec<[f32; 2]>);

/// Draws `display` in `cell`, and returns the view buttons' presses as
/// events. `views` are the display's views and `view` the one shown.
#[expect(clippy::too_many_arguments, reason = "the cell's inputs")]
pub fn draw(
    ui: &mut Ui,
    theme: &Theme,
    cell: Rect,
    id: Id,
    enabled: bool,
    display: &DeviceDisplay,
    panel: &DevicePanel,
    events: &mut Vec<UiEvent>,
) {
    let views = &panel.description.views;
    let c = &theme.colors;
    let painter = ui.painter_at(cell);
    // Switched off, the curves turn grey (observed).
    let ink = if enabled { c.accent.0 } else { c.inactive.0 };
    let view = display.view.as_deref();
    let collapsed = view == Some(COLLAPSED);
    // The data area leaves the meter's strip on the right.
    let meter = display.reading(GAIN_REDUCTION);
    let area = if meter.is_some() {
        Rect::from_min_max(
            cell.min,
            pos2(cell.right() - METER_WIDTH - 2.0, cell.bottom()),
        )
    } else {
        cell
    };
    if view == Some(ACTIVITY) {
        activity(ui, &painter, area, id, ink, display);
    } else if !collapsed {
        plot(&painter, theme, area, ink, display);
        crate::particles::draw(ui, theme, area, (id, enabled), display, panel, events);
    }
    if let Some(reading) = meter {
        let strip = Rect::from_min_max(
            pos2(cell.right() - METER_WIDTH - 1.0, cell.top() + 1.0),
            pos2(cell.right() - 1.0, cell.bottom() - 1.0),
        );
        painter.rect_filled(strip, 0.0, c.window.0);
        let depth = (-reading.value / METER_RANGE_DB).clamp(0.0, 1.0);
        let fill = Rect::from_min_size(strip.min, vec2(strip.width(), strip.height() * depth));
        painter.rect_filled(fill, 0.0, ink);
    }
    modulator_readings(&painter, theme, cell, ink, display);
    if views.len() > 1 {
        view_buttons(ui, theme, cell, display, views, enabled, events);
    }
}

/// What a modulator's display shows beside its curves (WP-5.11; the
/// modulation spec, R24 to R26): the Envelope Follower's Direct and S.C. meters in
/// dB from -60 to 0, a bar for a modulator's level, and the LED that flashes
/// on each start, in the cell's top-right corner.
fn modulator_readings(
    painter: &Painter,
    theme: &Theme,
    cell: Rect,
    ink: Color32,
    display: &DeviceDisplay,
) {
    let c = &theme.colors;
    let strip = |at: f32, share: f32| {
        let rect = Rect::from_min_max(
            pos2(at, cell.top() + 4.0),
            pos2(at + METER_WIDTH, cell.bottom() - 4.0),
        );
        painter.rect_filled(rect, 0.0, c.window.0);
        let fill = Rect::from_min_max(
            pos2(
                rect.left(),
                rect.bottom() - rect.height() * share.clamp(0.0, 1.0),
            ),
            rect.max,
        );
        painter.rect_filled(fill, 0.0, ink);
    };
    let mut left = cell.left() + 4.0;
    for name in ["Direct", "S.C."] {
        if let Some(reading) = display.reading(name) {
            strip(left, (reading.value + 60.0) / 60.0);
            left += METER_WIDTH + 4.0;
        }
    }
    if let Some(level) = display.reading("Level") {
        strip(cell.right() - METER_WIDTH - 4.0, level.value / 100.0);
    }
    if let Some(trigger) = display.reading("Trigger") {
        let at = pos2(cell.right() - METER_WIDTH - 14.0, cell.top() + 8.0);
        if trigger.value > 0.5 {
            painter.circle_filled(at, 3.0, ink);
        } else {
            painter.circle_stroke(at, 3.0, Stroke::new(1.0, c.separator.0));
        }
    }
}

/// The series and markers: spectra filled, curves as lines, the band
/// responses faint, and dots where markers sit.
fn plot(painter: &Painter, theme: &Theme, area: Rect, ink: Color32, display: &DeviceDisplay) {
    let c = &theme.colors;
    for series in display.series.iter().filter(|s| !s.values.is_empty()) {
        let at = |i: usize, v: f32| point(area, series, series.x_at(i), v);
        if series.name == SPECTRUM {
            let wide = area.width() / series.values.len().max(1) as f32;
            for (i, &v) in series.values.iter().enumerate() {
                let top = at(i, v);
                let bar = Rect::from_min_max(
                    pos2(top.x - wide / 2.0, top.y),
                    pos2(top.x + wide / 2.0, area.bottom()),
                );
                painter.rect_filled(bar, 0.0, c.text_dim.0.gamma_multiply(0.45));
            }
            continue;
        }
        let colour = if series.detail {
            ink.gamma_multiply(0.4)
        } else {
            ink
        };
        let line: Vec<Pos2> = series
            .values
            .iter()
            .enumerate()
            .map(|(i, &v)| at(i, v))
            .collect();
        painter.add(Shape::line(line, Stroke::new(1.5, colour)));
    }
    // The particles of a particle field are the handles it draws itself.
    for marker in display
        .markers
        .iter()
        .filter(|m| m.series != crate::particles::PARTICLES)
    {
        let Some(series) = display.series(&marker.series) else {
            continue;
        };
        let at = point(area, series, marker.x, marker.y);
        painter.circle(at, 3.0, ink, Stroke::new(1.0, c.text.0));
    }
}

/// The Activity view: the input's level and the gain reduction, scrolling
/// to the left.
fn activity(
    ui: &mut Ui,
    painter: &Painter,
    area: Rect,
    id: Id,
    ink: Color32,
    display: &DeviceDisplay,
) {
    let history_id = id.with("history");
    let mut history: History = ui.data(|d| d.get_temp(history_id)).unwrap_or_default();
    if let (Some(input), Some(gain)) = (display.reading(INPUT), display.reading(GAIN_REDUCTION)) {
        history.0.push([input.value, gain.value]);
        let excess = history.0.len().saturating_sub(HISTORY);
        history.0.drain(..excess);
    }
    let step = area.width() / HISTORY as f32;
    let x = |i: usize| area.right() - step * (history.0.len() - 1 - i) as f32;
    // The input from -60 dB to 0 up the cell; the reduction hangs from its
    // top, over the same 24 dB as the meter.
    let level = |db: f32| area.bottom() - area.height() * ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let input: Vec<Pos2> = history
        .0
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(x(i), level(v[0])))
        .collect();
    painter.add(Shape::line(
        input,
        Stroke::new(1.0, ink.gamma_multiply(0.5)),
    ));
    for (i, v) in history.0.iter().enumerate() {
        let depth = (-v[1] / METER_RANGE_DB).clamp(0.0, 1.0) * area.height();
        painter.line_segment(
            [pos2(x(i), area.top()), pos2(x(i), area.top() + depth)],
            Stroke::new(step.max(1.0), ink),
        );
    }
    ui.data_mut(|d| d.insert_temp(history_id, history));
}

/// The buttons that choose the display's view, along the cell's top-right
/// corner.
fn view_buttons(
    ui: &mut Ui,
    theme: &Theme,
    cell: Rect,
    display: &DeviceDisplay,
    views: &[String],
    enabled: bool,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let device = display.device;
    let right = cell.right() - METER_WIDTH - 6.0;
    for (i, view) in views.iter().enumerate() {
        let from = right - VIEW_BUTTON.x * (views.len() - i) as f32;
        let rect = Rect::from_min_size(pos2(from, cell.top() + 3.0), VIEW_BUTTON - vec2(2.0, 0.0));
        let button = view_button_id(device, view);
        let response = ui.interact(rect, button, Sense::click());
        crate::focus::register(ui.ctx(), button, bl_shell::Area::Detail, None);
        let chosen = display.view.as_deref() == Some(view.as_str());
        let pressed = response.clicked()
            || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if pressed && !chosen {
            events.push(UiEvent::Invoke(Invocation::new(
                "device.set_display_view",
                json!({ "device": device, "view": view }),
            )));
        }
        let face = match (chosen, enabled) {
            (true, true) => c.accent.0,
            (true, false) => c.inactive.0,
            _ => c.control.0,
        };
        let ink = if chosen { c.accent_text.0 } else { c.text.0 };
        ui.painter().rect_filled(rect, theme.sizes.corner, face);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            view_label(view),
            theme.small_font(),
            ink,
        );
        let name = format!("{view} view");
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Button);
            node.set_label(name);
            node.set_selected(chosen);
        });
    }
}
